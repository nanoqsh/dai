mod gemini;
mod http;
mod provider;
mod responses;

use {
    crate::{gemini::Gemini, provider::*, responses::OpenRouter},
    futures_lite::{StreamExt, future},
    std::error,
};

type Run<T> = Result<T, Box<dyn error::Error + Send + Sync>>;

pub fn main() -> Run<()> {
    future::block_on(run())
}

async fn run() -> Run<()> {
    let instructions = "You are an autonomous agent. You have access to tools. Use tools when necessary to accomplish the user's task. When the task is complete, finish the run";

    let tools = [Tool {
        name: "weather".into(),
        description: "Read current weather for the default (user's) location".into(),
        parameters: Parameters {},
    }];

    let mut provider: Box<dyn Provider> = if true {
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

    let message = "Привет! Какая сегодня погода?";
    let mut events = provider.req(Request::Message(message.into()));
    while let Some(event) = events.next().await {
        match event {
            Event::Error(e) => println!("event error: {e}"),
            Event::Reasoning { id, delta } => println!("event reasoning ({id:?}): {delta}"),
            Event::Message { id, delta } => println!("event message ({id:?}): {delta}"),
            Event::Call {
                id,
                name,
                arguments,
            } => println!("event call ({id}): {name} {arguments}"),
        }
    }

    Ok(())
}

pub struct Tool {
    name: Box<str>,
    description: Box<str>,
    #[expect(dead_code)]
    parameters: Parameters,
}

pub struct Parameters {}

#[derive(Clone, Copy)]
pub struct Config<'scope> {
    key: &'scope str,
    model: &'scope str,
    instructions: &'scope str,
    tools: &'scope [Tool],
}
