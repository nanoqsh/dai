use {
    blocking::Unblock,
    futures_lite::io::{AsyncRead, BufReader},
    serde_json::Value,
    std::{error, io::Read},
    ureq::{
        Agent, BodyReader,
        config::Config,
        http::{HeaderValue, StatusCode},
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
        if res.status() != StatusCode::OK {
            eprintln!("non ok status: {}", res.status());
            let json = res
                .headers()
                .get("Content-Type")
                .is_some_and(|content_type| content_type == "application/json");

            let body = blocking::unblock(|| -> Run<String> {
                let mut body = String::new();
                res.into_body().into_reader().read_to_string(&mut body)?;
                Ok(body)
            })
            .await?;

            return if json {
                let value: Value = serde_json::from_str(&body)?;
                Err(format!("body: {value:#}").into())
            } else {
                Err(format!("body: {body}").into())
            };
        }

        Ok(Response {
            body: res.into_body().into_reader(),
        })
    }
}

pub struct Response {
    body: BodyReader<'static>,
}

impl Response {
    pub fn into_reader(self) -> BufReader<impl AsyncRead> {
        BufReader::new(Unblock::new(self.body))
    }
}
