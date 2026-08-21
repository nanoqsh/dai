use {
    futures_lite::stream::{self, Stream},
    std::{error, fmt, pin::Pin},
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
    let fu = async {
        let sender = Sender(send);
        if let Err(e) = f(&sender).await {
            sender.send(Event::Error(e)).await;
        }
    };

    stream::stop_after_future(recv, fu)
}

pub struct Sender(async_channel::Sender<Event>);

impl Sender {
    pub async fn send(&self, event: Event) {
        _ = self.0.send(event).await;
    }
}

pub trait Provider {
    fn req(&mut self, req: Request) -> Events<'_>;
}

pub enum Request {
    Message(String),
    CallOutput { call_id: Box<str>, output: Box<str> },
}

#[derive(Debug)]
pub enum Event {
    Error(Error),
    Reasoning(Delta),
    Message(Delta),
    Call(Call),
}

#[derive(Debug)]
pub enum Error {
    Std(Box<dyn error::Error + Send + Sync>),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Std(s) => s.fmt(f),
        }
    }
}

impl<E> From<E> for Error
where
    E: Into<Box<dyn error::Error + Send + Sync>>,
{
    fn from(e: E) -> Self {
        Self::Std(e.into())
    }
}

#[derive(Debug)]
pub struct Delta {
    content_index: u64,
    item_id: Box<str>,
    delta: String,
}

impl Delta {
    pub fn new<I, D>(content_index: u64, item_id: I, delta: D) -> Option<Self>
    where
        I: Into<Box<str>>,
        D: Into<String>,
    {
        let delta = delta.into();
        if delta.is_empty() {
            None
        } else {
            Some(Self {
                content_index,
                item_id: item_id.into(),
                delta,
            })
        }
    }

    pub fn done<I>(content_index: u64, item_id: I) -> Self
    where
        I: Into<Box<str>>,
    {
        Self {
            content_index,
            item_id: item_id.into(),
            delta: String::new(),
        }
    }

    pub fn delta(&self) -> Option<&str> {
        if self.delta.is_empty() {
            None
        } else {
            Some(&self.delta)
        }
    }
}

#[derive(Debug)]
pub struct Call {
    pub call_id: Box<str>,
    pub name: Box<str>,
    pub arguments: String,
}
