//! A data structure for working with paginated endpoints

use {
    std::vec,
    futures::TryStreamExt as _,
    serde::{
        Deserialize,
        de::DeserializeOwned
    },
    crate::{
        Client,
        Error
    }
};

#[derive(Deserialize)]
struct PaginationInfo {
    cursor: String
}

#[derive(Deserialize)]
struct PaginatedResult<T> {
    data: Vec<T>,
    pagination: PaginationInfo
}

pub(crate) fn stream<'a, T: DeserializeOwned>(client: &'a Client, uri: String, query: Vec<(String, String)>) -> impl futures::stream::Stream<Item = Result<T, Error>> + 'a {
    futures::stream::try_unfold(None, move |cursor| {
        let uri_clone = uri.clone();
        let query_clone = query.clone();
        async move {
            let params = query_clone.into_iter().chain(cursor.map(|cursor| vec![(format!("after"), cursor)]).unwrap_or_default());
            let PaginatedResult { data, pagination }: PaginatedResult<T> = client.get_raw(&uri_clone, params).await?;
            if data.is_empty() {
                Ok::<_, Error>(None)
            } else {
                Ok(Some((futures::stream::iter(data.into_iter().map(Ok)), Some(pagination.cursor))))
            }
        }
    }).try_flatten()
}
