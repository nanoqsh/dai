use {
    crate::{Config, http::Http, provider::*},
    futures_lite::AsyncBufReadExt,
    serde::{Deserialize, Serialize},
    serde_json::Value,
    std::{borrow::Cow, error},
    ureq::http::HeaderValue,
};

type Run<T> = Result<T, Box<dyn error::Error + Send + Sync>>;

pub struct Gemini {
    key: HeaderValue,
    http: Http,
    model: Box<str>,
    // tools: Box<RawValue>,
    previous_interaction_id: Box<str>,
}

impl Gemini {
    pub fn new(conf: Config<'_>) -> Run<Self> {
        let key = HeaderValue::from_str(conf.key)?;
        let http = Http::new();

        // let tools =
        //    tools_json(conf.tools).map_err(|e| format!("failed to serialize tools: {e}"))?;

        Ok(Self {
            key,
            http,
            model: conf.model,
            previous_interaction_id: Box::default(),
        })
    }

    fn req(&mut self, req: Request) -> Events<'_> {
        let input = match &req {
            Request::Message(message) => Input::Text(message),
            Request::CallOutput { name, output, .. } => {
                let result = match serde_json::from_str(output) {
                    Ok(result) => result,
                    Err(e) => return error(e),
                };

                Input::FunctionResult {
                    ty: "function_result",
                    name,
                    result,
                }
            }
        };

        let req = HttpRequest {
            model: &self.model,
            input,
            stream: true,
            previous_interaction_id: if self.previous_interaction_id.is_empty() {
                None
            } else {
                Some(&self.previous_interaction_id)
            },
        };

        let body = match serde_json::to_string(&req) {
            Ok(body) => body,
            Err(e) => return error(e),
        };

        Box::pin(events(async |send| {
            let res = self
                .http
                .post(
                    "https://generativelanguage.googleapis.com/v1beta/interactions",
                    &[
                        ("x-goog-api-key", self.key.clone()),
                        ("Content-Type", HeaderValue::from_static("application/json")),
                    ],
                    &[("alt", "sse")],
                    body,
                )
                .await?;

            let mut step = Step::Empty;
            let mut event = String::new();
            let mut line = String::new();
            let mut read = res.into_reader();
            while {
                line.clear();
                read.read_line(&mut line).await? != 0
            } {
                let line = line.trim_ascii();
                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if let Some(e) = line.strip_prefix("event: ") {
                    if e == "done" {
                        break;
                    }

                    event.clear();
                    event.push_str(e);
                    continue;
                }

                let Some(s) = line.strip_prefix("data: ") else {
                    eprintln!("undefined={line}");
                    continue;
                };

                if let Some(event) =
                    parse_event(s, &event, &mut self.previous_interaction_id, &mut step)?
                {
                    send.send(event).await;
                }
            }

            Ok(())
        }))
    }
}

impl Provider for Gemini {
    fn req(&mut self, req: Request) -> Events<'_> {
        self.req(req)
    }
}

#[derive(Serialize)]
struct HttpRequest<'src> {
    model: &'src str,
    input: Input<'src>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_interaction_id: Option<&'src str>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Input<'src> {
    Text(&'src str),
    FunctionResult {
        #[serde(rename = "type")]
        ty: &'static str,
        name: &'src str,
        result: Value,
    },
}

#[derive(Deserialize)]
struct InteractionEvent<'src> {
    #[serde(borrow)]
    interaction: Interaction<'src>,
}

#[derive(Deserialize)]
struct Interaction<'src> {
    id: &'src str,
}

enum Step {
    Empty,
    Thought,
    ModelOutput,
    FunctionCall,
}

fn parse_event(
    s: &str,
    event: &str,
    previous_interaction_id: &mut Box<str>,
    step: &mut Step,
) -> Run<Option<Event>> {
    match event {
        "interaction.created" => {
            let event: InteractionEvent<'_> = serde_json::from_str(s)?;
            *previous_interaction_id = event.interaction.id.into();
        }
        "interaction.status_update" => {}
        "interaction.completed" => {}
        "step.start" => {
            #[derive(Deserialize)]
            struct Event<'src> {
                #[serde(borrow)]
                step: StepType<'src>,
            }

            #[derive(Deserialize)]
            struct StepType<'src> {
                #[serde(rename = "type")]
                ty: &'src str,
            }

            let event: Event<'_> = serde_json::from_str(s)?;
            *step = match event.step.ty {
                "thought" => Step::Thought,
                "model_output" => Step::ModelOutput,
                "function_call" => Step::FunctionCall,
                unknown => {
                    eprintln!("unknown step type: {unknown}");
                    Step::Empty
                }
            };
        }
        "step.delta" => {
            #[derive(Deserialize)]
            struct DeltaEvent<'src> {
                index: u64,
                #[serde(borrow)]
                delta: DeltaType<'src>,
            }

            #[derive(Deserialize)]
            #[serde(tag = "type")]
            enum DeltaType<'src> {
                #[serde(rename = "text")]
                Text { text: Cow<'src, str> },
                #[serde(rename = "thought_signature")]
                ThoughtSignature { signature: &'src str },
                #[serde(other)]
                Unknown,
            }

            let event: DeltaEvent<'_> = serde_json::from_str(s)?;
            match (step, event.delta) {
                (Step::ModelOutput, DeltaType::Text { text }) => {
                    return Ok(Delta::new(event.index, "", text).map(Event::Message));
                }
                (Step::Thought, DeltaType::ThoughtSignature { signature }) => _ = signature,
                (_, DeltaType::Unknown) => eprintln!("unknown delta type"),
                _ => {}
            }
        }
        "step.stop" => {}
        unknown => eprintln!("unknown event: {unknown}"),
    }

    Ok(None)
}
