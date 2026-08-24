use {
    futures_lite::stream::{self, Stream},
    serde_json::value::RawValue,
    std::{error, fmt, future, pin::Pin},
};

pub type Events<'pro> = Pin<Box<dyn Stream<Item = Event> + Send + 'pro>>;

pub fn error<E>(e: E) -> Events<'static>
where
    E: Into<Error>,
{
    Box::pin(stream::once(Event::Error(e.into())))
}

pub fn events<F>(f: F) -> impl Stream<Item = Event>
where
    F: AsyncFnOnce(&Sender) -> Result<(), Error>,
{
    let (send, recv) = async_channel::bounded(1);
    let fu = stream::once_future(async {
        let sender = Sender(send);
        if let Err(e) = f(&sender).await {
            sender.send(Event::Error(e)).await;
        }

        drop(sender);
        future::pending().await
    });

    stream::or(fu, recv)
}

pub struct Sender(async_channel::Sender<Event>);

impl Sender {
    pub async fn send(&self, event: Event) {
        self.0.send(event).await.expect("send event");
    }
}

pub trait Provider {
    fn req(&mut self, req: Request) -> Events<'_>;
}

pub enum Request {
    Message(String),
    CallOutputs(Vec<CallOutput>),
}

pub struct CallOutput {
    pub id: Box<str>,
    pub name: Box<str>,
    pub output: Box<RawValue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Id(u64);

impl From<u64> for Id {
    fn from(u: u64) -> Self {
        Self(u)
    }
}

#[derive(Debug)]
pub enum Event {
    Error(Error),
    Reasoning {
        id: Id,
        delta: String,
    },
    Message {
        id: Id,
        delta: String,
    },
    Call {
        id: Box<str>,
        name: Box<str>,
        arguments: Box<RawValue>,
    },
}

#[derive(Debug)]
pub struct Error(Box<dyn error::Error + Send + Sync>);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<E> From<E> for Error
where
    E: Into<Box<dyn error::Error + Send + Sync>>,
{
    fn from(e: E) -> Self {
        Self(e.into())
    }
}
