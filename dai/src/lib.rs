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
    let tools = [Tool {
        name: "weather".into(),
        description: "Read current weather for the default (user's) location".into(),
        parameters: Parameters {},
    }];

    let mut provider: Box<dyn Provider> = if true {
        let key = env!("GEMINI_API_KEY");
        let conf = Config {
            key,
            model: "gemini-3.6-flash".into(),
            tools: &tools,
        };

        Box::new(Gemini::new(conf)?)
    } else {
        let key = env!("OPENROUTER_API_KEY");
        let conf = Config {
            key,
            model: "openai/gpt-oss-120b".into(),
            tools: &tools,
        };

        Box::new(OpenRouter::new(conf)?)
    };

    let message = "Привет! Какая сегодня погода?";
    let mut events = provider.req(Request::Message(message.into()));
    while let Some(event) = events.next().await {
        println!("EVENT: {event:?}");
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

pub struct Config<'scope> {
    key: &'scope str,
    model: Box<str>,
    tools: &'scope [Tool],
}
