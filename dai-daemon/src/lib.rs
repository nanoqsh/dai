mod gemini;
mod handler;
mod http;
mod provider;
mod responses;

use {
    crate::{
        gemini::Gemini,
        handler::{Call, Func},
        provider::*,
        responses::OpenRouter,
    },
    futures_lite::{StreamExt, future},
    serde::Serialize,
    serde_json::{Map, Value, value::RawValue},
    std::{
        collections::HashMap,
        env::{self, VarError},
        error,
        fmt::Write,
        path::PathBuf,
    },
};

type Run<T> = Result<T, Box<dyn error::Error + Send + Sync>>;

pub fn main() -> Run<()> {
    future::block_on(run())
}

async fn run() -> Run<()> {
    let path_parameter = Parameter {
        name: "path".into(),
        description: "Relative file path".into(),
        ty: PrimitiveType::String,
    };

    let tools = &Tools::new([
        Tool {
            name: "fs.read".into(),
            description: "Read a text file from the agent's working directory".into(),
            parameters: vec![path_parameter.clone()].into_boxed_slice(),
            func: handler::make(async || { Ok(()) }, &[]),
        },
        Tool {
            name: "fs.write".into(),
            description: "Write text to a file in the agent's working directory. Creates the file if it does not exist and replaces its existing contents".into(),
            parameters: vec![
                path_parameter,
                Parameter {
                    name: "content".into(),
                    description: "Text to write to the file".into(),
                    ty: PrimitiveType::String,
                },
            ]
            .into_boxed_slice(),
            func: handler::make::<_, fn(PathBuf, str), _>(
                fs_write,
                &["path", "content"],
            ),
        },
    ]);

    let instructions = &instructions();
    let mut provider: Box<dyn Provider> = if false {
        let key = env!("GEMINI_API_KEY");
        let conf = Config {
            key,
            model: "gemini-3.6-flash",
            instructions,
            tools,
        };

        Box::new(Gemini::new(conf)?)
    } else {
        let key = env!("OPENROUTER_API_KEY");
        let conf = Config {
            key,
            model: "openai/gpt-oss-120b",
            instructions,
            tools,
        };

        Box::new(OpenRouter::new(conf)?)
    };

    let message =
        "Напиши файл .py скрипт на питоне 3.14: ввод строк в сосоли и вывод самой длинной";

    let mut req = Some(Request::Message(message.into()));
    while let Some(currreq) = req.take() {
        let mut events = provider.req(currreq);
        while let Some(event) = events.next().await {
            match event {
                Event::Error(e) => println!("event error: {e}"),
                Event::Reasoning { id, delta } => println!("event reasoning ({id:?}): {delta}"),
                Event::Message { id, delta } => println!("event message ({id:?}): {delta}"),
                Event::Call { id, name, args } => {
                    println!("event call ({id}): {name} {args}");

                    let Some(tool) = tools.get(&name) else {
                        eprintln!("no tool with name: {name}");
                        continue;
                    };

                    let call_output = match (tool.func)(&args).await {
                        Ok(output) => CallOutput { id, name, output },
                        Err(e) => CallOutput {
                            id,
                            name,
                            output: Value::Object({
                                let mut map = Map::new();
                                map.insert("error".into(), Value::String(e));
                                map
                            }),
                        },
                    };

                    // TODO
                    req = Some(Request::CallOutputs(vec![call_output]));
                }
            }
        }
    }

    Ok(())
}

async fn fs_write(path: PathBuf, content: &str) -> Call<()> {
    async_fs::write(&path, content)
        .await
        .map_err(|e| format!("failed to write in file {}: {e}", path.display()))
}

pub struct Tool {
    name: Box<str>,
    description: Box<str>,
    parameters: Box<[Parameter]>,
    func: Func,
}

#[derive(Clone)]
pub struct Parameter {
    name: Box<str>,
    description: Box<str>,
    ty: PrimitiveType,
}

#[derive(Clone, Copy)]
pub enum PrimitiveType {
    String,
}

impl PrimitiveType {
    fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
        }
    }
}

pub struct Tools {
    tools: Vec<Tool>,
    byname: HashMap<Box<str>, usize>,
}

impl Tools {
    pub fn new<I>(tools: I) -> Self
    where
        I: IntoIterator<Item = Tool>,
    {
        let mut byname = HashMap::new();
        let tools = tools
            .into_iter()
            .inspect(|t| {
                let index = byname.len();
                byname.insert(t.name.clone(), index);
            })
            .collect();

        Self { tools, byname }
    }

    pub fn get(&self, name: &str) -> Option<&Tool> {
        let index = *self.byname.get(name)?;
        self.tools.get(index)
    }

    pub fn to_json(&self) -> Run<Box<RawValue>> {
        #[derive(Serialize)]
        struct WriteTool<'src> {
            #[serde(rename = "type")]
            ty: &'static str,
            name: &'src str,
            description: &'src str,
            parameters: WriteParameters<'src>,
        }

        #[derive(Serialize)]
        struct WriteParameters<'src> {
            #[serde(rename = "type")]
            ty: &'static str,
            properties: HashMap<&'src str, WriteProperty<'src>>,
            required: Vec<&'src str>,
            #[serde(rename = "additionalProperties")]
            additional_properties: bool,
        }

        #[derive(Serialize)]
        struct WriteProperty<'src> {
            #[serde(rename = "type")]
            ty: &'static str,
            description: &'src str,
        }

        let write: Vec<_> = self
            .tools
            .iter()
            .map(|tool| {
                let properties = tool
                    .parameters
                    .iter()
                    .map(|p| {
                        let prop = WriteProperty {
                            ty: p.ty.as_str(),
                            description: &p.description,
                        };

                        (p.name.as_ref(), prop)
                    })
                    .collect();

                WriteTool {
                    ty: "function",
                    name: &tool.name,
                    description: &tool.description,
                    parameters: WriteParameters {
                        ty: "object",
                        properties,
                        required: tool.parameters.iter().map(|p| p.name.as_ref()).collect(),
                        additional_properties: false,
                    },
                }
            })
            .collect();

        let s = serde_json::to_string(&write)?;
        Ok(RawValue::from_string(s)?)
    }
}

#[derive(Clone, Copy)]
pub struct Config<'scope> {
    key: &'scope str,
    model: &'scope str,
    instructions: &'scope str,
    tools: &'scope Tools,
}

fn instructions() -> String {
    let mut s = String::from(
        "[BASIC]\nYou are an autonomous agent. You have access to listed tools. Use tools when necessary to accomplish the user's task. Do not call unlisted tools. When the task is complete, finish the run",
    );

    let username = username();
    let userinfo = [
        Some(("os", env::consts::OS)),
        username.as_deref().map(|name| ("username", name)),
    ]
    .into_iter()
    .flatten();

    _ = write!(&mut s, "\n[USERINFO]");
    for (key, value) in userinfo {
        _ = write!(&mut s, "\n{key}: {value}");
    }

    s
}

fn username() -> Option<String> {
    match env::var("USER") {
        Ok(name) => return Some(name),
        Err(VarError::NotPresent) => {}
        Err(VarError::NotUnicode(_)) => return None,
    }

    cfg_select! {
        unix => {
            use nix::unistd::{User, Uid};

            let uid = Uid::current();
            if let Some(user) = User::from_uid(uid).ok().flatten() {
                return Some(user.name);
            }
        }
        _ => {}
    }

    None
}
