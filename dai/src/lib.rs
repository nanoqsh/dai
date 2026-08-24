mod gemini;
mod http;
mod provider;
mod responses;

use {
    crate::{gemini::Gemini, provider::*, responses::OpenRouter},
    futures_lite::{StreamExt, future},
    serde::Serialize,
    serde_json::value::RawValue,
    std::{
        collections::HashMap,
        env, error, fs,
        path::{Component, Path},
    },
};

type Run<T> = Result<T, Box<dyn error::Error + Send + Sync>>;

pub fn main() -> Run<()> {
    future::block_on(run())
}

async fn run() -> Run<()> {
    let instructions = "You are an autonomous agent. You have access to tools. Use tools when necessary to accomplish the user's task. When the task is complete, finish the run";

    let path_parameter = Parameter {
        name: "path".into(),
        description: "Relative file path".into(),
        ty: PrimitiveType::String,
    };

    let tools = Tools::new([
        Tool {
            name: "fs.read".into(),
            description: "Read a text file from the agent's working directory".into(),
            parameters: vec![path_parameter.clone()].into_boxed_slice(),
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
        },
    ]);

    let mut provider: Box<dyn Provider> = if false {
        let key = env!("GEMINI_API_KEY");
        let conf = Config {
            key,
            model: "gemini-3.6-flash",
            instructions,
            tools: &tools,
        };

        Box::new(Gemini::new(conf)?)
    } else {
        let key = env!("OPENROUTER_API_KEY");
        let conf = Config {
            key,
            model: "openai/gpt-oss-120b",
            instructions,
            tools: &tools,
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
                Event::Call {
                    id,
                    name,
                    arguments,
                } => {
                    println!("event call ({id}): {name} {arguments}");

                    let Some(tool) = tools.get(&name) else {
                        eprintln!("no tool with name: {name}");
                        continue;
                    };

                    if tool.name.as_ref() == "fs.write" {
                        let arguments = serde_json::to_value(arguments)?;
                        let path = arguments["path"].as_str().unwrap_or_default();
                        let content = arguments["content"].as_str().unwrap_or_default();

                        let path = Path::new(path);
                        if path.is_absolute() {
                            eprintln!("path is absolute: {}", path.display());
                            continue;
                        }

                        for component in path.components() {
                            match component {
                                Component::Prefix(_) => return Err("prefix".into()),
                                Component::RootDir => return Err("root".into()),
                                Component::CurDir => return Err("curdir".into()),
                                Component::ParentDir => return Err("parentdir".into()),
                                Component::Normal(_) => {}
                            }
                        }

                        let hostdir = env::current_dir()?.join(path);
                        if let Some(parent) = hostdir.parent() {
                            fs::create_dir_all(parent)?;
                        }

                        fs::write(hostdir, content)?;
                        req = Some(Request::CallOutputs(vec![CallOutput {
                            id,
                            name,
                            output: RawValue::from_string(String::from("{}"))?,
                        }]));
                    }
                }
            }
        }
    }

    Ok(())
}

pub struct Tool {
    name: Box<str>,
    description: Box<str>,
    parameters: Box<[Parameter]>,
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
