use {
    crate::{Config, http::Http, provider::*},
    futures_lite::AsyncBufReadExt,
    rand::RngExt,
    serde::{Deserialize, Serialize},
    serde_json::value::RawValue,
    std::{borrow::Cow, collections::HashMap, error},
    ureq::http::HeaderValue,
};

type Run<T> = Result<T, Box<dyn error::Error + Send + Sync>>;

pub struct OpenRouter(ResponsesApi);

impl OpenRouter {
    pub fn new(conf: Config<'_>) -> Run<Self> {
        let api = ResponsesApi::new(
            conf,
            Mode::Stateless {
                inputs: vec![],
                agent_messages: HashMap::new(),
            },
        )?;

        Ok(Self(api))
    }
}

impl Provider for OpenRouter {
    fn req(&mut self, req: Request) -> Events<'_> {
        self.0.req(req)
    }
}

struct ResponsesApi {
    auth: HeaderValue,
    http: Http,
    model: Box<str>,
    instructions: Box<str>,
    session_id: Box<str>,
    tools: Box<RawValue>,
    mode: Mode,
}

impl ResponsesApi {
    fn new(conf: Config<'_>, mode: Mode) -> Run<Self> {
        let key = conf.key;
        let auth = HeaderValue::try_from(format!("Bearer {key}"))
            .map_err(|_| format!("invalid auth key: {key}"))?;

        let http = Http::new();

        let mut rng = rand::rng();
        let session_id: String = (0..7)
            .map(|_| rng.sample(rand::distr::Alphanumeric) as char)
            .collect();

        let tools = conf
            .tools
            .to_json()
            .map_err(|e| format!("failed to serialize tools: {e}"))?;

        Ok(Self {
            auth,
            http,
            model: conf.model.into(),
            instructions: conf.instructions.into(),
            session_id: session_id.into_boxed_str(),
            tools,
            mode,
        })
    }

    fn req(&mut self, req: Request) -> Events<'_> {
        if let Err(e) = self.mode.push_request(req) {
            return error(e);
        }

        let req = HttpRequest {
            model: &self.model,
            session_id: &self.session_id,
            stream: true,
            instructions: &self.instructions,
            input: self.mode.inputs(),
            tools: &self.tools,
            previous_response_id: self.mode.previous_response_id(),
        };

        let body = match serde_json::to_string(&req) {
            Ok(body) => body,
            Err(e) => return error(e),
        };

        Box::pin(events(async move |send| {
            let res = self
                .http
                .post(
                    "https://openrouter.ai/api/v1/responses",
                    &[
                        ("Authorization", self.auth.clone()),
                        ("Content-Type", HeaderValue::from_static("application/json")),
                    ],
                    &[],
                    body,
                )
                .await?;

            let mut items = Items(HashMap::new());
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

                let Some(s) = line.strip_prefix("data: ") else {
                    eprintln!("undefined={line}");
                    continue;
                };

                if s == "[DONE]" {
                    break;
                }

                let previous_response_id = match self.mode.previous_response_id_mut() {
                    Some(prev) => prev,
                    None => &mut Box::default(),
                };

                if let Some(event) = parse_event(s, previous_response_id, &mut items)? {
                    self.mode.push_event(&event)?;
                    send.send(event).await;
                }
            }

            self.mode.commit_agent_messages()?;
            Ok(())
        }))
    }
}

enum Mode {
    Stateless {
        inputs: Vec<Box<RawValue>>,
        agent_messages: HashMap<Id, String>,
    },
    #[expect(dead_code)]
    Statefull {
        inputs: Vec<Box<RawValue>>,
        previous_response_id: Box<str>,
    },
}

impl Mode {
    fn push_request(&mut self, req: Request) -> Run<()> {
        let value = input_json(Input::from_request(req)?)?;
        let (Self::Stateless { inputs, .. } | Self::Statefull { inputs, .. }) = self;
        inputs.extend(value);
        Ok(())
    }

    fn push_event(&mut self, event: &Event) -> Run<()> {
        let Self::Stateless {
            inputs,
            agent_messages,
        } = self
        else {
            return Ok(());
        };

        if !matches!(event, Event::Message { .. }) {
            commit_agent_messages(inputs, agent_messages)?;
        }

        match &event {
            Event::Message { id, delta } => agent_messages.entry(*id).or_default().push_str(delta),
            Event::Call {
                id,
                name,
                arguments,
            } => {
                let (id, call_id) = id.split_once(':').ok_or("invalid id")?;
                let input = Input::Call(FunctionCall {
                    id,
                    call_id,
                    name,
                    arguments: arguments.get(),
                });

                inputs.extend(input_json(input)?);
            }
            Event::Reasoning { .. } | Event::Error(_) => {}
        }

        Ok(())
    }

    fn commit_agent_messages(&mut self) -> Run<()> {
        let Self::Stateless {
            inputs,
            agent_messages,
        } = self
        else {
            return Ok(());
        };

        commit_agent_messages(inputs, agent_messages)
    }

    fn inputs(&self) -> &[Box<RawValue>] {
        let (Self::Stateless { inputs, .. } | Self::Statefull { inputs, .. }) = self;
        inputs
    }

    fn previous_response_id(&self) -> Option<&str> {
        match self {
            Self::Stateless { .. } => None,
            Self::Statefull {
                previous_response_id,
                ..
            } if previous_response_id.is_empty() => None,
            Self::Statefull {
                previous_response_id,
                ..
            } => Some(previous_response_id),
        }
    }

    fn previous_response_id_mut(&mut self) -> Option<&mut Box<str>> {
        match self {
            Self::Stateless { .. } => None,
            Self::Statefull {
                previous_response_id,
                ..
            } => Some(previous_response_id),
        }
    }
}

fn commit_agent_messages(
    inputs: &mut Vec<Box<RawValue>>,
    agent_messages: &mut HashMap<Id, String>,
) -> Run<()> {
    for (_, message) in agent_messages.drain() {
        let input = Input::AgentMessage(message);
        inputs.extend(input_json(input)?);
    }

    Ok(())
}

#[derive(Serialize)]
struct HttpRequest<'src> {
    model: &'src str,
    session_id: &'src str,
    stream: bool,
    instructions: &'src str,
    input: &'src [Box<RawValue>],
    tools: &'src RawValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<&'src str>,
}

#[derive(Deserialize)]
struct Header<'src> {
    #[serde(rename = "type")]
    ty: &'src str,
}

#[derive(Debug, Deserialize)]
struct Response<'src> {
    id: &'src str,
    #[expect(dead_code)]
    output: Vec<Output<'src>>,
    error: Option<&'src RawValue>,
    #[expect(dead_code)]
    previous_response_id: Option<&'src str>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ResponseEvent<'src> {
    #[serde(borrow)]
    response: Response<'src>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Output<'src> {
    #[expect(dead_code)]
    #[serde(rename = "reasoning")]
    Reasoning {
        status: &'src str,
        #[serde(default)]
        content: Vec<Content<'src>>,
        #[serde(default)]
        summary: Vec<&'src RawValue>,
    },
    #[expect(dead_code)]
    #[serde(rename = "message")]
    Message {
        status: &'src str,
        role: &'src str,
        #[serde(default)]
        content: Vec<Content<'src>>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: &'src str,
        status: &'src str,
        call_id: &'src str,
        name: &'src str,
        arguments: Cow<'src, str>,
    },
}

#[derive(Debug, Deserialize)]
struct Content<'src> {
    #[expect(dead_code)]
    text: Cow<'src, str>,
    #[expect(dead_code)]
    #[serde(borrow, default)]
    annotations: Vec<&'src RawValue>,
}

#[derive(Default, Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

#[derive(Deserialize)]
struct DeltaEvent<'src> {
    item_id: &'src str,
    delta: Cow<'src, str>,
}

struct Items(HashMap<Box<str>, Id>);

impl Items {
    fn get(&mut self, id: &str) -> Id {
        let new = self.0.len() as u64;
        *self.0.entry(id.into()).or_insert_with(|| Id::from(new))
    }
}

fn parse_event(s: &str, response_id: &mut Box<str>, items: &mut Items) -> Run<Option<Event>> {
    let Header { ty } =
        serde_json::from_str(s).inspect_err(|_| eprintln!("failed to parse json: {s}"))?;

    println!("[{ty}] {s}");

    match ty {
        "response.created" => {
            let event: ResponseEvent<'_> = serde_json::from_str(s)?;
            *response_id = event.response.id.into();
        }
        "response.in_progress" => {}
        "response.completed" => {
            let event: ResponseEvent<'_> = serde_json::from_str(s)?;
            if let Some(e) = event.response.error {
                eprintln!("response error: {e}");
            }

            if let Some(usage) = event.response.usage {
                println!("usage input tokens: {}", usage.input_tokens);
                println!("usage output tokens: {}", usage.output_tokens);
                println!("usage total tokens: {}", usage.total_tokens);
            }
        }
        "response.output_item.added" => {}
        "response.output_item.done" => {
            #[derive(Deserialize)]
            struct Done<'src> {
                #[serde(borrow)]
                item: Output<'src>,
            }

            let event: Done<'_> = serde_json::from_str(s)?;

            if let Output::FunctionCall {
                id,
                status,
                call_id,
                name,
                arguments,
            } = event.item
            {
                if status != "completed" {
                    eprintln!("status should be completed, but it's {status}");
                }

                return Ok(Some(Event::Call {
                    id: format!("{id}:{call_id}").into_boxed_str(),
                    name: name.into(),
                    arguments: RawValue::from_string(arguments.into_owned())?,
                }));
            }
        }
        "response.content_part.added" => {}
        "response.content_part.done" => {}
        "response.reasoning_text.delta" => {
            let event: DeltaEvent<'_> = serde_json::from_str(s)?;
            return Ok(Some(Event::Reasoning {
                id: items.get(event.item_id),
                delta: event.delta.into_owned(),
            }));
        }
        "response.reasoning_text.done" => {}
        "response.output_text.delta" => {
            let event: DeltaEvent<'_> = serde_json::from_str(s)?;
            return Ok(Some(Event::Message {
                id: items.get(event.item_id),
                delta: event.delta.into_owned(),
            }));
        }
        "response.output_text.done" => {}
        "response.function_call_arguments.delta" => {}
        "response.function_call_arguments.done" => {}
        unknown => eprintln!("unknown type: {unknown}"),
    }

    Ok(None)
}

enum Input<'src> {
    UserMessage(String),
    AgentMessage(String),
    CallOutputs(Vec<FunctionCallOutput>),
    Call(FunctionCall<'src>),
}

impl Input<'_> {
    fn from_request(req: Request) -> Run<Self> {
        match req {
            Request::Message(s) => Ok(Self::UserMessage(s)),
            Request::CallOutputs(outputs) => Ok(Self::CallOutputs(
                outputs
                    .into_iter()
                    .map(|o| -> Run<_> {
                        let (_, call_id) = o.id.split_once(':').ok_or("invalid id")?;
                        Ok(FunctionCallOutput {
                            call_id: call_id.into(),
                            output: o.output.get().into(),
                        })
                    })
                    .collect::<Run<_>>()?,
            )),
        }
    }
}

#[derive(Serialize)]
struct FunctionCallOutput {
    call_id: Box<str>,
    output: Box<str>,
}

#[derive(Serialize)]
struct FunctionCall<'src> {
    id: &'src str,
    call_id: &'src str,
    name: &'src str,
    arguments: &'src str,
}

fn input_json(input: Input<'_>) -> Run<Vec<Box<RawValue>>> {
    #[derive(Serialize)]
    #[serde(tag = "type")]
    enum WriteInput<'src> {
        #[serde(rename = "message")]
        Message { role: Role, content: &'src str },
        #[serde(rename = "function_call_output")]
        FunctionCallOutput(FunctionCallOutput),
        #[serde(rename = "function_call")]
        FunctionCall(FunctionCall<'src>),
    }

    #[derive(Serialize)]
    #[serde(rename_all = "lowercase")]
    enum Role {
        User,
        Assistant,
    }

    let one = |w| {
        let s = serde_json::to_string(&w)?;
        let json = RawValue::from_string(s)?;
        Ok(vec![json])
    };

    match input {
        Input::UserMessage(s) => one(WriteInput::Message {
            role: Role::User,
            content: &s,
        }),
        Input::AgentMessage(s) => one(WriteInput::Message {
            role: Role::Assistant,
            content: &s,
        }),
        Input::CallOutputs(outputs) => outputs
            .into_iter()
            .map(|o| {
                let s = serde_json::to_string(&WriteInput::FunctionCallOutput(o))?;
                Ok(RawValue::from_string(s)?)
            })
            .collect(),
        Input::Call(call) => one(WriteInput::FunctionCall(call)),
    }
}
