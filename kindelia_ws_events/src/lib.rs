use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use warp::ws::{Message, WebSocket};
use warp::{Filter, Rejection, Reply};

use std::convert::Infallible;
use std::str::FromStr;

/// Main trait of the lib.
/// 
/// Specifies a enum struct that must be serializable by serde
/// and that must have a `Discriminant`, a type which is used 
/// to convert between filtrable tags in string to the actual item enum type
pub trait SerializableWithDiscriminant
where
  Self: Send + Clone + serde::Serialize,
{
  type Discriminant: Send + From<Self> + FromStr<Err = String> + Eq;
}

#[derive(serde::Deserialize)]
struct QueryParams {
  tags: Option<String>,
}

struct Query {
  tags: Vec<String>,
}

/// Main function of the lib. This will spawn a tokio runtime and
/// block in a task that contains a websocket server. 
/// 
/// This websocket server is responsible to share with all of
/// its clients (broadcast) the enum item sended by the channel `ws_tx`.
/// 
/// This enum item must implement `SerializableWithDiscriminant` in 
/// order to be serde::Serializable, so the json encoding can be performed
/// and in order to have `Discriminant` type, that is used to specify filtrable
/// string tags for the clients.
pub fn ws_loop<T: 'static>(port: u16, ws_tx: broadcast::Sender<T>)
where
  T: SerializableWithDiscriminant,
{
  let runtime = tokio::runtime::Runtime::new().unwrap();
  runtime.block_on(async move {
    ws_server(port, ws_tx).await;
  });
}

async fn ws_server<T: 'static>(port: u16, ws_tx: broadcast::Sender<T>)
where
  T: SerializableWithDiscriminant,
{
  let ws_route = warp::ws()
    .and(with_rx(ws_tx.clone()))
    .and(warp::query::<QueryParams>().map(parse_query))
    .and_then(ws_handler);
  warp::serve(ws_route).run(([127, 0, 0, 1], port)).await;
}

fn parse_query(query: QueryParams) -> Query {
  let tags = match query.tags {
    Some(tags) => tags.split(',').map(str::to_string).collect(),
    None => vec![],
  };
  Query { tags }
}

fn with_rx<T>(
  ws_tx: broadcast::Sender<T>,
) -> impl Filter<Extract = (broadcast::Sender<T>,), Error = Infallible> + Clone
where
  T: SerializableWithDiscriminant,
{
  warp::any().map(move || ws_tx.clone())
}

async fn ws_handler<T: 'static>(
  ws: warp::ws::Ws,
  ws_tx: broadcast::Sender<T>,
  query: Query,
) -> Result<impl Reply, Rejection>
where
  T: SerializableWithDiscriminant,
{
  Ok(ws.on_upgrade(move |socket| client_connection(socket, ws_tx, query.tags)))
}

async fn client_connection<T>(
  ws: WebSocket,
  ws_tx: broadcast::Sender<T>,
  tags: Vec<String>,
) where
  T: SerializableWithDiscriminant,
{
  let (mut client_ws_sender, _) = ws.split();
  let mut ws_rx = ws_tx.subscribe();
  let mut count = 0;

  while let Ok(event) = ws_rx.recv().await {
    let tags: Result<Vec<T::Discriminant>, String> =
      tags.iter().map(|tag| T::Discriminant::from_str(tag)).collect();

    if let Ok(tags) = tags {
      if tags.is_empty() || tags.contains(&(event.clone().into())) {
        let json_stringfied = serde_json::to_string(&event).unwrap();
        if let Err(err) =
          client_ws_sender.send(Message::text(json_stringfied)).await
        {
          eprintln!("Could not send message through websocket: {}", err);
          count += 1;
        } else {
          count = 0;
        };
        // After 10 consecutive fails we close the connection
        if count == 10 {
          break;
        };
      }
    } else {
      break;
    }
  }

  eprintln!("Disconnected");
}
