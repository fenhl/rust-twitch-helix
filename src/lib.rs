//! A Rust client for the [twitch.tv Helix API](https://dev.twitch.tv/docs/api).

#![deny(missing_docs, rust_2018_idioms, unused, unused_import_braces, /*unused_lifetimes,*/ /*TODO uncomment once https://github.com/rust-lang/rust/issues/78522 is fixed*/ unused_qualifications, warnings)]

use {
    std::{
        borrow::{
            Borrow,
            Cow,
        },
        fmt,
        mem,
        sync::Arc,
    },
    async_trait::async_trait,
    chrono::prelude::*,
    derive_more::From,
    futures::TryFutureExt as _,
    itertools::{
        EitherOrBoth,
        Itertools as _,
    },
    reqwest::{
        IntoUrl,
        StatusCode,
    },
    serde::{
        Deserialize,
        de::DeserializeOwned,
    },
    tokio::{
        sync::RwLock,
        time::sleep,
    },
};

pub mod model;
pub mod paginated;

pub(crate) const HELIX_BASE_URL: &str = "https://api.twitch.tv/helix";

/// An enum that contains all the different kinds of errors that can occur in the library.
#[derive(Debug, From)]
#[allow(missing_docs)]
pub enum Error {
    #[from(ignore)]
    ExactlyOne(bool),
    HttpStatus(reqwest::Error, reqwest::Result<String>),
    InvalidHeaderValue(reqwest::header::InvalidHeaderValue),
    Reqwest(reqwest::Error),
    ResponseJson(serde_json::Error, String),
}

impl Error {
    fn is_invalid_oauth_token(&self) -> bool {
        match self {
            Error::HttpStatus(e, _) | Error::Reqwest(e) => e.status().map_or(false, |code| code == StatusCode::UNAUTHORIZED), //TODO check response body to make sure
            Error::ExactlyOne(_) | Error::InvalidHeaderValue(_) | Error::ResponseJson(_, _) => false,
        }
    }

    fn is_spurious_network_error(&self) -> bool {
        match self {
            Error::HttpStatus(e, _) | Error::Reqwest(e) => e.status().map_or(false, |code| !code.is_client_error()),
            Error::ExactlyOne(_) | Error::InvalidHeaderValue(_) | Error::ResponseJson(_, _) => false,
        }
    }
}

impl<I: Iterator> From<itertools::ExactlyOneError<I>> for Error {
    fn from(mut e: itertools::ExactlyOneError<I>) -> Error {
        Error::ExactlyOne(e.next().is_none())
    }
}

#[async_trait]
trait ResponseExt {
    async fn json_with_text_in_error<T: DeserializeOwned>(self) -> Result<T, Error>;
}

#[async_trait]
impl ResponseExt for reqwest::Response {
    async fn json_with_text_in_error<T: DeserializeOwned>(self) -> Result<T, Error> {
        let text = self.text().await?;
        serde_json::from_str(&text).map_err(|e| Error::ResponseJson(e, text))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ExactlyOne(true) => write!(f, "tried to get exactly one item from an iterator but it was empty"),
            Error::ExactlyOne(false) => write!(f, "tried to get exactly one item from an iterator but it contained multiple items"),
            Error::HttpStatus(e, Ok(body)) => write!(f, "{}, body:\n\n{}", e, body),
            Error::HttpStatus(e, Err(_)) => e.fmt(f),
            Error::InvalidHeaderValue(e) => e.fmt(f),
            Error::Reqwest(e) => e.fmt(f),
            Error::ResponseJson(e, body) => write!(f, "{}, body:\n\n{}", e, body),
        }
    }
}

/// Info required to use the Twitch API.
///
/// Can be constructed from a client secret and/or an OAuth token, see the docs on the methods for details.
pub struct Credentials(EitherOrBoth<(String, String), String>); // left = (client_secret, scopes), right = oauth_token

impl Credentials {
    /// Use the given client secret to generate a new OAuth token.
    pub fn from_client_secret<S: fmt::Display, U: fmt::Display, I: IntoIterator<Item = U>>(client_secret: S, scopes: I) -> Credentials {
        Credentials(EitherOrBoth::Left((client_secret.to_string(), scopes.into_iter().join(" "))))
    }

    /// Use the given OAuth token. When the token expires, the error is passed to the caller.
    pub fn from_oauth_token(oauth_token: impl fmt::Display) -> Credentials {
        Credentials(EitherOrBoth::Right(oauth_token.to_string()))
    }

    /// Use the given OAuth token. When the token expires, use the given client secret to generate a new OAuth token.
    pub fn from_client_secret_and_oauth_token<S: fmt::Display, U: fmt::Display, I: IntoIterator<Item = U>, T: fmt::Display>(client_secret: S, scopes: I, oauth_token: T) -> Credentials {
        Credentials(EitherOrBoth::Both((client_secret.to_string(), scopes.into_iter().join(" ")), oauth_token.to_string()))
    }

    fn set_token(&mut self, token: String) {
        self.0 = match mem::replace(&mut self.0, EitherOrBoth::Right(String::default())) {
            EitherOrBoth::Left((client_secret, scopes)) | EitherOrBoth::Both((client_secret, scopes), _) => EitherOrBoth::Both((client_secret, scopes), token),
            EitherOrBoth::Right(_) => EitherOrBoth::Right(token),
        };
    }
}

#[derive(Deserialize)]
struct CredentialsResponse {
    access_token: String,
}

/// The entry point to the API.
pub struct Client<'a> {
    client: reqwest::Client,
    client_id: Cow<'a, str>,
    /// If we're currently being rate limited, this has the time when the API can be called again.
    rate_limit_reset: Option<DateTime<Utc>>,
    credentials: Arc<RwLock<Credentials>>,
}

impl<'a> Client<'a> {
    /// Constructs a new `Client` for accessing the [Helix API](https://dev.twitch.tv/docs/api).
    ///
    /// The `user_agent` parameter is used as the `User-Agent` header for all requests. It must be a `&'static str` for performance reasons.
    ///
    /// The remaining parameters of this constructor reflect that [as of April 30, 2020, all Helix endpoints require OAuth tokens](https://discuss.dev.twitch.tv/t/requiring-oauth-for-helix-twitch-api-endpoints/23916).
    pub fn new(user_agent: &'static str, client_id: impl Into<Cow<'a, str>>, credentials: Credentials) -> Result<Client<'a>, Error> {
        let client_id = client_id.into();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::USER_AGENT, reqwest::header::HeaderValue::from_static(user_agent));
        headers.insert("Client-ID", reqwest::header::HeaderValue::from_str(&client_id)?);
        Ok(Client {
            client_id,
            client: reqwest::Client::builder()
                .default_headers(headers)
                .build()?,
            rate_limit_reset: None,
            credentials: Arc::new(RwLock::new(credentials)),
        })
    }

    /*
    pub(crate) async fn get<U: fmt::Display, T: DeserializeOwned>(&self, url: U) -> Result<T, Error> {
        self.get_abs(&format!("{}{}", HELIX_BASE_URL, url)).await
    }

    pub(crate) async fn get_abs<U: IntoUrl, T: DeserializeOwned>(&self, url: U) -> Result<T, Error> {
        self.get_abs_query(url, &Vec::<(String, String)>::default()).await
    }
    */

    pub(crate) async fn get_query<U: fmt::Display, K: AsRef<str>, V: AsRef<str>, Q: IntoIterator, T: DeserializeOwned>(&self, url: U, query: Q) -> Result<T, Error>
    where Q::Item: Borrow<(K, V)> {
        self.get_abs_query(&format!("{}{}", HELIX_BASE_URL, url), query).await
    }

    pub(crate) async fn get_abs_query<U: IntoUrl, K: AsRef<str>, V: AsRef<str>, Q: IntoIterator, T: DeserializeOwned>(&self, url: U, query: Q) -> Result<T, Error>
    where Q::Item: Borrow<(K, V)> {
        Ok(self.get_raw::<_, _, _, _, ResponseData<_>>(url, query).await?.data)
    }

    pub(crate) async fn get_raw<U: IntoUrl, K: AsRef<str>, V: AsRef<str>, Q: IntoIterator, T: DeserializeOwned>(&self, url: U, query: Q) -> Result<T, Error>
    where Q::Item: Borrow<(K, V)> {
        let mut token = self.get_oauth_token(None).await?;
        let mut url = url.into_url()?;
        url.query_pairs_mut().extend_pairs(query);
        Ok(loop {
            // wait for rate limit
            if let Some(rate_limit_reset) = self.rate_limit_reset {
                if let Ok(duration) = (rate_limit_reset - Utc::now()).to_std() {
                    sleep(duration).await;
                    continue
                }
            }
            // send request
            let response_data = self.client.get(url.clone())
                .bearer_auth(&token)
                .send().map_err(Error::Reqwest)
                .and_then(|resp| async {
                    match resp.error_for_status_ref() {
                        Ok(_) => Ok(resp),
                        Err(e) => Err(Error::HttpStatus(e, resp.text().await)),
                    }
                })
                .await;
            match response_data {
                Ok(data) => break data.json_with_text_in_error().await?,
                Err(e) => if e.is_spurious_network_error() {
                    // simply try again
                } else if e.is_invalid_oauth_token() {
                    token = self.get_oauth_token(Some(e)).await?;
                } else {
                    return Err(e)
                },
            }
            let response = self.client.get(url.clone())
                .bearer_auth(&token)
                .send().await?;
            if let Err(e) = response.error_for_status_ref() {
                return Err(Error::HttpStatus(e, response.text().await))
            }
            break response.json_with_text_in_error().await?
        })
    }

    /// Returns an OAuth token from the credentials with which this `Client` was constructed. If no token is cached, a new one is created by authenticating with Twitch.
    ///
    /// The optional parameter `from_error` can be passed to handle an “invalid OAuth token” error by reauthenticating. Other errors are returned transparently.
    pub async fn get_oauth_token(&self, from_error: Option<Error>) -> Result<String, Error> {
        if from_error.as_ref().map_or(false, |e| !e.is_invalid_oauth_token()) {
            // return non-auth errors transparently
            return Err(from_error.expect("just checked"))
        }
        let response = match (from_error, &self.credentials.read().await.0) {
            // we have a cached token and no auth error, so just return that
            (None, EitherOrBoth::Right(oauth_token)) | (None, EitherOrBoth::Both(_, oauth_token)) => return Ok(oauth_token.to_owned()),
            // there was an auth error but we only have a token, no client ID/secret, so we're unable to reauth
            (Some(e), EitherOrBoth::Right(_)) => return Err(e),
            // there was an auth error, so reauth
            (_, EitherOrBoth::Left((client_secret, scopes))) | (Some(_), EitherOrBoth::Both((client_secret, scopes), _)) => {
                self.client.post("https://id.twitch.tv/oauth2/token")
                    .query(&[
                        ("client_id", &*self.client_id),
                        ("client_secret", client_secret),
                        ("grant_type", "client_credentials"),
                        ("scope", scopes),
                    ])
                    .send().await?
            }
        };
        if let Err(e) = response.error_for_status_ref() {
            return Err(Error::HttpStatus(e, response.text().await))
        }
        let new_token = response.json_with_text_in_error::<CredentialsResponse>().await?.access_token;
        self.credentials.write().await.set_token(new_token.clone()); // cache the new token
        Ok(new_token)
    }
}

#[derive(Debug, Deserialize)]
struct ResponseData<T> {
    data: T,
}
