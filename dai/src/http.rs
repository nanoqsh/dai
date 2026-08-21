use {
    blocking::Unblock,
    futures_lite::io::{AsyncRead, BufReader},
    std::error,
    ureq::{
        Agent, BodyReader,
        config::Config,
        http::{HeaderMap, HeaderValue, StatusCode},
    },
};

type Run<T> = Result<T, Box<dyn error::Error + Send + Sync>>;

#[derive(Clone)]
pub struct Http {
    http: Agent,
}

impl Http {
    pub fn new() -> Self {
        let conf = Config::builder().http_status_as_error(false).build();
        let http = Agent::new_with_config(conf);
        Self { http }
    }

    pub async fn post(
        &self,
        uri: &str,
        headers: &[(&str, HeaderValue)],
        queries: &[(&str, &str)],
        body: String,
    ) -> Run<Response> {
        let mut b = self.http.post(uri);
        for (key, value) in headers {
            b = b.header(*key, value);
        }

        for (key, value) in queries {
            b = b.query(key, value);
        }

        let res = blocking::unblock(|| b.send(body)).await?;
        let (parts, body) = res.into_parts();
        Ok(Response {
            status: parts.status,
            headers: parts.headers,
            body: body.into_reader(),
        })
    }
}

pub struct Response {
    status: StatusCode,
    headers: HeaderMap<HeaderValue>,
    body: BodyReader<'static>,
}

impl Response {
    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn headers(&self) -> &HeaderMap<HeaderValue> {
        &self.headers
    }

    pub fn into_reader(self) -> BufReader<impl AsyncRead> {
        BufReader::new(Unblock::new(self.body))
    }
}
