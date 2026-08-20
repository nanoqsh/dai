use {
    futures_lite::stream,
    serde::{Deserialize, Serialize},
    serde_json::{Value, value::RawValue},
    std::{
        borrow::Cow,
        error,
        io::{BufReader, prelude::BufRead},
    },
    ureq::{config::Config, http::StatusCode},
};

type Run<T> = Result<T, Box<dyn error::Error>>;

pub fn run() -> Run<()> {
    let key = env!("OPENROUTER_API_KEY");
    let header = format!("Bearer {key}");

    let conf = Config::builder().http_status_as_error(false).build();
    let http = ureq::Agent::new_with_config(conf);

    let req = HttpRequest {
        model: "openai/gpt-oss-120b",
        session_id: "1234",
        stream: true,
        instructions: "You are an autonomous agent. You have access to tools. Use tools when necessary to accomplish the user's task. When the task is complete, finish the run",
        input: vec![
            Input::Message {
                role: Role::User,
                content: "Привет! Какая сегодня погода?",
            },
            // Input::FunctionCall(FunctionCall {
            //     call_id: "call-db93a5c4-ab9a-4139-bbfb-b676c9366186",
            //     name: "weather",
            //     arguments: "{}".to_owned(),
            // }),
            // Input::FunctionCallOutput(FunctionCallOutput {
            //     call_id: "call-db93a5c4-ab9a-4139-bbfb-b676c9366186",
            //     output: "23°C, clear",
            // }),
            // Input::Message {
            //     role: Role::Assistant,
            //     content: "Привет! Сегодня **23 °C**, ясно. 🌞",
            // },
            // Input::Message {
            //     role: Role::User,
            //     content: "спосибо",
            // },
        ],
        tools: vec![Tool {
            ty: ToolType::Function,
            name: "weather",
            description: "Read current weather for the default (user's) location",
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }),
        }],
        previous_response_id: None,
    };

    let body = serde_json::to_string(&req)?;
    let mut res = http
        .post("https://openrouter.ai/api/v1/responses")
        .header("Authorization", &header)
        .header("Content-Type", "application/json")
        .send(body)?;

    if res.status() != StatusCode::OK {
        eprintln!("non ok status: {}", res.status());
        eprintln!("body: {}", res.body_mut().read_to_string()?);
        return Ok(());
    }

    let body = BufReader::new(res.into_body().into_reader());
    for res in body.lines() {
        let line = res?;
        if line.is_empty() || line.starts_with(':') {
            continue;
        }

        if let Some(data) = line.strip_prefix("data: ") {
            if data == "[DONE]" {
                break;
            }

            let Header { ty } = serde_json::from_str(data)
                .inspect_err(|_| eprintln!("failed to parse json: {data}"))?;

            println!("new event with type: {ty}");

            match ty {
                "response.created" => {
                    let event: ResponseEvent<'_> = serde_json::from_str(data)?;
                    println!("response={:?}", &event.response);
                }
                "response.in_progress" => {
                    let event: ResponseEvent<'_> = serde_json::from_str(data)?;
                    println!("response={:?}", &event.response);
                }
                "response.completed" => {
                    let event: ResponseEvent<'_> = serde_json::from_str(data)?;
                    println!("response={:?}", &event.response);
                }
                "response.output_item.added" => {
                    #[derive(Deserialize)]
                    struct Added<'src> {
                        output_index: u64,
                        #[serde(borrow)]
                        item: Output<'src>,
                    }

                    let event: Added<'_> = serde_json::from_str(data)?;
                    println!("output_index={}", event.output_index);
                    println!("item={:?}", &event.item);
                }
                "response.output_item.done" => {
                    #[derive(Deserialize)]
                    struct Done<'src> {
                        output_index: u64,
                        #[serde(borrow)]
                        item: Output<'src>,
                    }

                    let event: Done<'_> = serde_json::from_str(data)?;
                    println!("output_index={}", event.output_index);
                    println!("item={:?}", event.item);

                    if let Output::FunctionCall {
                        id: _,
                        status,
                        call_id,
                        name,
                        arguments,
                    } = event.item
                    {
                        if status != "completed" {
                            eprintln!("status should be completed, but it's {status}");
                        }

                        let out = Event::Call(Call {
                            call_id: call_id.into(),
                            name: name.into(),
                            arguments: arguments.into_owned(),
                        });

                        println!("EVENT: {out:?}");
                    }
                }
                "response.content_part.added" => {
                    #[derive(Deserialize)]
                    struct Added<'src> {
                        output_index: u64,
                        item_id: &'src str,
                        #[serde(borrow)]
                        part: &'src RawValue,
                    }

                    let event: Added<'_> = serde_json::from_str(data)?;
                    println!("output_index={}", event.output_index);
                    println!("item_id={}", event.item_id);
                    println!("part={}", event.part.get());
                }
                "response.content_part.done" => {
                    #[derive(Deserialize)]
                    struct Done<'src> {
                        item_id: &'src str,
                        #[serde(borrow)]
                        part: &'src RawValue,
                        text: Option<&'src str>,
                    }

                    let event: Done<'_> = serde_json::from_str(data)?;
                    println!("item_id={}", event.item_id);
                    println!("part={}", event.part.get());
                    println!("text={:?}", event.text);
                }
                "response.reasoning_text.delta" => {
                    let event: DeltaEvent<'_> = serde_json::from_str(data)?;
                    println!("content_index={}", event.content_index);
                    println!("item_id={}", event.item_id);
                    println!("delta={}", event.delta);

                    let out = Event::Reasoning(Delta {
                        content_index: event.content_index,
                        item_id: event.item_id.into(),
                        delta: event.delta.into_owned(),
                    });

                    println!("EVENT: {out:?}");
                }
                "response.reasoning_text.done" => {
                    #[derive(Deserialize)]
                    struct Done<'src> {
                        content_index: u64,
                        item_id: &'src str,
                        text: Cow<'src, str>,
                    }

                    let event: Done<'_> = serde_json::from_str(data)?;
                    println!("content_index={}", event.content_index);
                    println!("item_id={}", event.item_id);
                    println!("text={}", event.text);
                }
                "response.output_text.delta" => {
                    let event: DeltaEvent<'_> = serde_json::from_str(data)?;
                    println!("content_index={}", event.content_index);
                    println!("item_id={}", event.item_id);
                    println!("delta={}", event.delta);

                    let out = Event::Message(Delta {
                        content_index: event.content_index,
                        item_id: event.item_id.into(),
                        delta: event.delta.into_owned(),
                    });

                    println!("EVENT: {out:?}");
                }
                "response.output_text.done" => {
                    #[derive(Deserialize)]
                    struct Done<'src> {
                        item_id: &'src str,
                        text: Cow<'src, str>,
                    }

                    let event: Done<'_> = serde_json::from_str(data)?;
                    println!("item_id={}", event.item_id);
                    println!("text={}", event.text);
                }
                "response.function_call_arguments.delta" => {
                    #[derive(Deserialize)]
                    struct Event<'src> {
                        item_id: &'src str,
                        output_index: u64,
                        delta: Cow<'src, str>,
                    }

                    let event: Event<'_> = serde_json::from_str(data)?;
                    println!("item_id={}", event.item_id);
                    println!("output_index={}", event.output_index);
                    println!("delta={}", event.delta);
                }
                "response.function_call_arguments.done" => {
                    #[derive(Deserialize)]
                    struct Event<'src> {
                        item_id: &'src str,
                        output_index: u64,
                        arguments: Cow<'src, str>,
                    }

                    let event: Event<'_> = serde_json::from_str(data)?;
                    println!("item_id={}", event.item_id);
                    println!("output_index={}", event.output_index);
                    println!("arguments={}", event.arguments);
                }
                unknown => eprintln!("unknown type: {unknown}"),
            }
        } else {
            eprintln!("undefined={line}");
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct HttpRequest<'src> {
    model: &'src str,
    session_id: &'src str,
    stream: bool,
    instructions: &'src str,
    input: Vec<Input<'src>>,
    tools: Vec<Tool<'src>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_response_id: Option<&'src str>,
}

#[derive(Serialize)]
struct Tool<'src> {
    #[serde(rename = "type")]
    ty: ToolType,
    name: &'src str,
    description: &'src str,
    parameters: Value,
}

#[derive(Serialize)]
enum ToolType {
    #[serde(rename = "function")]
    Function,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum Input<'src> {
    #[serde(rename = "message")]
    Message { role: Role, content: &'src str },
    #[serde(rename = "function_call")]
    FunctionCall(FunctionCall<'src>),
    #[serde(rename = "function_call_output")]
    FunctionCallOutput(FunctionCallOutput<'src>),
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    User,
    Assistant,
}

#[derive(Serialize)]
struct FunctionCall<'src> {
    call_id: &'src str,
    name: &'src str,
    arguments: String,
}

#[derive(Serialize)]
struct FunctionCallOutput<'src> {
    call_id: &'src str,
    output: &'src str,
}

#[derive(Deserialize)]
struct Header<'src> {
    #[serde(rename = "type")]
    ty: &'src str,
}

#[derive(Debug, Deserialize)]
struct Response<'src> {
    id: &'src str,
    model: &'src str,
    output: Vec<Output<'src>>,
    error: Option<&'src RawValue>,
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
    #[serde(rename = "reasoning")]
    Reasoning {
        id: &'src str,
        status: &'src str,
        #[serde(default)]
        content: Vec<Content<'src>>,
        #[serde(default)]
        summary: Vec<&'src RawValue>,
    },
    #[serde(rename = "message")]
    Message {
        id: &'src str,
        status: &'src str,
        role: &'src str,
        #[serde(default)]
        content: Vec<Content<'src>>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        #[expect(dead_code)]
        id: &'src str,
        status: &'src str,
        call_id: &'src str,
        name: &'src str,
        arguments: Cow<'src, str>,
    },
}

#[derive(Debug, Deserialize)]
struct Content<'src> {
    text: Cow<'src, str>,
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
    content_index: u64,
    item_id: &'src str,
    delta: Cow<'src, str>,
}

pub trait Provider {
    fn req(&mut self, req: Request) -> stream::Boxed<Event>;
}

pub enum Request {
    Message(String),
    CallOutput,
}

#[derive(Debug)]
pub enum Event {
    Error(Error),
    Reasoning(Delta),
    Message(Delta),
    Call(Call),
}

#[derive(Debug)]
pub enum Error {}

#[derive(Debug)]
pub struct Delta {
    content_index: u64,
    item_id: Box<str>,
    delta: String,
}

#[derive(Debug)]
pub struct Call {
    call_id: Box<str>,
    name: Box<str>,
    arguments: String,
}
