use ureq::{
    Agent, Body, Error,
    config::Config,
    http::{HeaderValue, Response},
};

pub struct Http {
    http: Agent,
}

impl Http {
    pub fn new() -> Self {
        let conf = Config::builder().http_status_as_error(false).build();
        let http = Agent::new_with_config(conf);
        Self { http }
    }

    pub fn post(
        &self,
        uri: &str,
        headers: &mut dyn Iterator<Item = (&str, HeaderValue)>,
        body: String,
    ) -> Result<Response<Body>, Error> {
        let mut b = self.http.post(uri);
        for (key, value) in headers {
            b = b.header(key, value);
        }

        b.send(body)
    }
}
