use {
    crate::{Config, http::Http, provider::*},
    futures_lite::AsyncBufReadExt,
    serde::{Deserialize, Serialize},
    serde_json::{Map, Value, value::RawValue},
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

        let tools = conf
            .tools
            .to_json()
            .map_err(|e| format!("failed to serialize tools: {e}"))?;

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
            Request::CallOutputs(outputs) => Input::FunctionResults(
                outputs
                    .iter()
                    .map(|o| FunctionResult {
                        ty: "function_result",
                        name: &o.name,
                        result: &o.output,
                    })
                    .collect(),
            ),
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

            let mut steps = Steps(HashMap::new());
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
                    parse_event(s, &event, &mut self.previous_interaction_id, &mut steps)?
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
    FunctionResults(Vec<FunctionResult<'src>>),
}

#[derive(Serialize)]
struct FunctionResult<'src> {
    #[serde(rename = "type")]
    ty: &'static str,
    name: &'src str,
    result: &'src Value,
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
    Thought,
    ModelOutput,
    FunctionCall {
        id: Box<str>,
        name: Box<str>,
        arguments: String,
    },
}

struct Steps(HashMap<Id, Step>);

impl Steps {
    fn insert(&mut self, id: Id, step: Step) {
        self.0.insert(id, step);
    }

    fn get(&mut self, id: Id) -> Option<&mut Step> {
        self.0.get_mut(&id)
    }

    fn remove(&mut self, id: Id) -> Option<Step> {
        self.0.remove(&id)
    }
}

fn parse_event(
    s: &str,
    event: &str,
    previous_interaction_id: &mut Box<str>,
    steps: &mut Steps,
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
            struct StartEvent<'src> {
                index: u64,
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
                    arguments: HashMap<Box<str>, Box<RawValue>>,
                },
                #[serde(other)]
                Unknown,
            }

            let event: StartEvent<'_> = serde_json::from_str(s)?;
            let id = Id::from(event.index);
            let step = match event.step {
                Kind::Thought => Step::Thought,
                Kind::ModelOutput => Step::ModelOutput,
                Kind::FunctionCall {
                    id,
                    name,
                    arguments,
                } => {
                    let arguments = if arguments.is_empty() {
                        String::new()
                    } else {
                        serde_json::to_string(&arguments)?
                    };

                    Step::FunctionCall {
                        id: id.into(),
                        name: name.into(),
                        arguments,
                    }
                }
                Kind::Unknown => return Err("unknown step type".into()),
            };

            steps.insert(id, step);
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
                #[serde(rename = "arguments_delta")]
                Arguments { arguments: Cow<'src, str> },
                #[serde(other)]
                Unknown,
            }

            let event: DeltaEvent<'_> = serde_json::from_str(s)?;
            let id = Id::from(event.index);
            let Some(step) = steps.get(id) else {
                return Err(format!("no step with index {}", event.index).into());
            };

            match (step, event.delta) {
                (Step::Thought, DeltaType::ThoughtSignature { signature }) => _ = signature,
                (Step::ModelOutput, DeltaType::Text { text }) => {
                    return Ok(Some(Event::Message {
                        id,
                        delta: text.into_owned(),
                    }));
                }
                (
                    Step::FunctionCall { arguments, .. },
                    DeltaType::Arguments { arguments: delta },
                ) => arguments.push_str(&delta),
                _ => eprintln!("unknown step types"),
            }
        }
        "step.stop" => {
            #[derive(Deserialize)]
            struct StopEvent {
                index: u64,
            }

            let event: StopEvent = serde_json::from_str(s)?;
            let id = Id::from(event.index);
            if let Some(Step::FunctionCall {
                id,
                name,
                arguments,
            }) = steps.remove(id)
            {
                let args = if arguments.is_empty() {
                    Value::Object(Map::new())
                } else {
                    serde_json::from_str(&arguments)?
                };

                return Ok(Some(Event::Call { id, name, args }));
            }
        }
        "error" => {
            #[derive(Deserialize)]
            struct ErrorEvent {
                error: ErrorMessage,
            }

            #[derive(Deserialize)]
            struct ErrorMessage {
                message: String,
            }

            let event: ErrorEvent = serde_json::from_str(s)?;
            return Ok(Some(Event::Error(Error::from(event.error.message))));
        }
        unknown => eprintln!("unknown event: {unknown}"),
    }

    Ok(None)
}
