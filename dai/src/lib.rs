use {
    serde::{Deserialize, Serialize},
    serde_json::value::RawValue,
    std::{
        borrow::Cow,
        error,
        io::{BufReader, prelude::BufRead},
    },
    ureq::{Agent, config::Config, http::StatusCode},
};

type Run<T> = Result<T, Box<dyn error::Error>>;

pub fn run() -> Run<()> {
    let key = env!("OPENROUTER_API_KEY");
    let header = format!("Bearer {key}");

    let conf = Config::builder().http_status_as_error(false).build();
    let http = Agent::new_with_config(conf);

    let req = Request {
        model: "openai/gpt-oss-120b",
        session_id: "1234",
        stream: true,
        instructions: "You are an autonomous agent. Use tools when necessary",
        input: vec![
            Input::Message {
                role: Role::User,
                content: "Привет! Какая сегодня погода?",
            },
            Input::FunctionCall(FunctionCall {
                call_id: "chatcmpl-tool-413befe9319d465ab967449233e2a9b3",
                name: "weather",
                arguments: "{}".to_owned(),
            }),
            Input::FunctionCallOutput(FunctionCallOutput {
                call_id: "chatcmpl-tool-413befe9319d465ab967449233e2a9b3",
                output: "23°C, clear",
            }),
            Input::Message {
                role: Role::Assistant,
                content: "Привет! Сегодня **23 °C**, ясно. 🌞",
            },
            Input::Message {
                role: Role::User,
                content: "спосибо",
            },
        ],
        tools: vec![Tool {
            ty: ToolType::Function,
            name: "weather",
            description: "Read current weather.",
            parameters: serde_json::json!({
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "required": [],
                    "additionalProperties": false
                }
            }),
        }],
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

            #[derive(Deserialize)]
            struct Header<'src> {
                #[serde(rename = "type")]
                ty: &'src str,
            }

            let Header { ty } = serde_json::from_str(data)
                .inspect_err(|_| eprintln!("failed to parse json: {data}"))?;

            println!("new event with type: {ty}");

            match ty {
                "response.created" => {
                    #[derive(Deserialize)]
                    struct Created<'src> {
                        #[serde(borrow)]
                        response: &'src RawValue,
                    }

                    let event: Created<'_> = serde_json::from_str(data)?;
                    println!("response={}", event.response.get());
                }
                "response.in_progress" => {
                    #[derive(Deserialize)]
                    struct Progress<'src> {
                        #[serde(borrow)]
                        response: &'src RawValue,
                    }

                    let event: Progress<'_> = serde_json::from_str(data)?;
                    println!("response={}", event.response.get());
                }
                "response.output_item.added" => {
                    #[derive(Deserialize)]
                    struct Added<'src> {
                        output_index: u64,
                        #[serde(borrow)]
                        item: &'src RawValue,
                    }

                    let event: Added<'_> = serde_json::from_str(data)?;
                    println!("output_index={}", event.output_index);
                    println!("item={}", event.item.get());
                }
                "response.output_item.done" => {
                    #[derive(Deserialize)]
                    struct Done<'src> {
                        output_index: u64,
                        #[serde(borrow)]
                        item: &'src RawValue,
                    }

                    let event: Done<'_> = serde_json::from_str(data)?;
                    println!("output_index={}", event.output_index);
                    println!("item={}", event.item.get());
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
                    #[derive(Deserialize)]
                    struct Delta<'src> {
                        content_index: u64,
                        item_id: &'src str,
                        delta: Cow<'src, str>,
                    }

                    let event: Delta<'_> = serde_json::from_str(data)?;
                    println!("content_index={}", event.content_index);
                    println!("item_id={}", event.item_id);
                    println!("delta={}", event.delta);
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
                    #[derive(Deserialize)]
                    struct Delta<'src> {
                        content_index: u64,
                        item_id: &'src str,
                        delta: Cow<'src, str>,
                    }

                    let event: Delta<'_> = serde_json::from_str(data)?;
                    println!("content_index={}", event.content_index);
                    println!("item_id={}", event.item_id);
                    println!("delta={}", event.delta);
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
                "response.completed" => {
                    #[derive(Deserialize)]
                    struct Completed<'src> {
                        #[serde(borrow)]
                        response: &'src RawValue,
                    }

                    let event: Completed<'_> = serde_json::from_str(data)?;
                    println!("response={}", event.response.get());
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
struct Request<'src> {
    model: &'src str,
    session_id: &'src str,
    stream: bool,
    instructions: &'src str,
    input: Vec<Input<'src>>,
    tools: Vec<Tool<'src>>,
}

#[derive(Serialize)]
struct Tool<'src> {
    #[serde(rename = "type")]
    ty: ToolType,
    name: &'src str,
    description: &'src str,
    parameters: serde_json::Value,
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
