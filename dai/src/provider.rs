use futures_lite::stream;

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
    pub content_index: u64,
    pub item_id: Box<str>,
    pub delta: String,
}

#[derive(Debug)]
pub struct Call {
    pub call_id: Box<str>,
    pub name: Box<str>,
    pub arguments: String,
}
