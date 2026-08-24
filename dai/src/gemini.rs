use {
    crate::{Config, Tool, http::Http, provider::*},
    futures_lite::AsyncBufReadExt,
    serde::{Deserialize, Serialize},
    serde_json::{Value, value::RawValue},
    std::{borrow::Cow, collections::HashMap, error},
    ureq::http::HeaderValue,
};

type Run<T> = Result<T, Box<dyn error::Error + Send + Sync>>;

pub struct Gemini {
    key: HeaderValue,
    http: Http,
    model: Box<str>,
    instructions: Box<str>,
    tools: Box<RawValue>,
    previous_interaction_id: Box<str>,
}

impl Gemini {
    pub fn new(conf: Config<'_>) -> Run<Self> {
        let key = HeaderValue::from_str(conf.key)?;
        let http = Http::new();

        let tools =
            tools_json(conf.tools).map_err(|e| format!("failed to serialize tools: {e}"))?;

        Ok(Self {
            key,
            http,
            model: conf.model.into(),
            instructions: conf.instructions.into(),
            tools,
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
            stream: true,
            system_instruction: &self.instructions,
            input,
            tools: &self.tools,
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
    stream: bool,
    system_instruction: &'src str,
    input: Input<'src>,
    tools: &'src RawValue,
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
    println!("[{event}] {s}");

    match event {
        "interaction.created" => {
            let event: InteractionEvent<'_> = serde_json::from_str(s)?;
            *previous_interaction_id = event.interaction.id.into();
        }
        "interaction.status_update" => {}
        "interaction.completed" => {}
        "step.start" => {
            #[derive(Deserialize)]
            struct StepEvent<'src> {
                #[serde(borrow)]
                step: Kind<'src>,
            }

            #[derive(Deserialize)]
            #[serde(tag = "type")]
            enum Kind<'src> {
                #[serde(rename = "thought")]
                Thought,
                #[serde(rename = "model_output")]
                ModelOutput,
                #[serde(rename = "function_call")]
                FunctionCall {
                    id: &'src str,
                    name: &'src str,
                    arguments: HashMap<Cow<'src, str>, &'src RawValue>,
                },
                #[serde(other)]
                Unknown,
            }

            let event: StepEvent<'_> = serde_json::from_str(s)?;
            let mut res = None;
            *step = match event.step {
                Kind::Thought => Step::Thought,
                Kind::ModelOutput => Step::ModelOutput,
                Kind::FunctionCall {
                    id,
                    name,
                    arguments,
                } => {
                    let arguments = serde_json::to_string(&arguments)?;
                    res = Some(Event::Call {
                        id: id.into(),
                        name: name.into(),
                        arguments,
                    });

                    Step::FunctionCall
                }
                Kind::Unknown => {
                    eprintln!("unknown step type");
                    Step::Empty
                }
            };

            return Ok(res);
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
                    return Ok(Some(Event::Message {
                        id: Id::from(event.index),
                        delta: text.into_owned(),
                    }));
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

fn tools_json(tools: &[Tool]) -> Run<Box<RawValue>> {
    #[derive(Serialize)]
    struct WriteTool<'src> {
        #[serde(rename = "type")]
        ty: &'static str,
        name: &'src str,
        description: &'src str,
        parameters: Value,
    }

    let write: Vec<_> = tools
        .iter()
        .map(|tool| WriteTool {
            ty: "function",
            name: &tool.name,
            description: &tool.description,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }),
        })
        .collect();

    let s = serde_json::to_string(&write)?;
    Ok(RawValue::from_string(s)?)
}
