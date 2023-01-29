use anyhow::{Context, Error, Result};
use futures_util::StreamExt;
use songbird::{
    input::{Input, Restartable},
    Songbird,
};
use std::{env, future::Future, sync::Arc};
use tokio::sync::mpsc;
use twilight_gateway::{Cluster, Event, Intents};
use twilight_http::Client as HttpClient;
use twilight_model::channel::Message;

type State = Arc<StateRef>;

#[derive(Debug)]
pub struct StateRef {
    http: HttpClient,
    songbird: Songbird,
}

fn spawn(fut: impl Future<Output = Result<()>> + Send + 'static, shutdown: mpsc::Sender<Error>) {
    tokio::spawn(async move {
        if let Err(why) = fut.await {
            let _ = shutdown.send(why).await;
        }
    });
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Initialize the tracing subscriber.
    tracing_subscriber::fmt::init();

    let (mut events, state) = {
        let token = env::var("DISCORD_TOKEN")?;

        let http = HttpClient::new(token.clone());
        let user_id = http.current_user().exec().await?.model().await?.id;

        let intents = Intents::GUILD_VOICE_STATES;
        let (cluster, events) = Cluster::new(token, intents).await?;
        cluster.up().await;

        let songbird = Songbird::twilight(Arc::new(cluster), user_id);

        (events, Arc::new(StateRef { http, songbird }))
    };

    let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

    while let Some((_, event)) = events.next().await {
        if let Ok(error) = shutdown_rx.try_recv() {
            return Err(error);
        }

        state.songbird.process(&event).await;

        if let Event::Ready(_) = event {
            spawn(join(Arc::clone(&state)), shutdown_tx.clone());
        }
    }

    Ok(())
}

async fn join(state: State) -> Result<(), Error> {
    let (_handle, status) = state
        .songbird
        .join(532298747284291584, 743945715683819661)
        .await;

    if status.is_ok() {
        tracing::info!("Successfully connected to discord channel");
    }

    status.context("Failed to join channel")
}

pub async fn leave(msg: Message, state: State) -> Result<()> {
    tracing::debug!(
        "leave command in channel {} by {}",
        msg.channel_id,
        msg.author.name
    );

    let guild_id = msg.guild_id.unwrap();

    state.songbird.leave(guild_id).await?;

    state
        .http
        .create_message(msg.channel_id)
        .content("Left the channel")?
        .exec()
        .await?;

    Ok(())
}

pub async fn play(msg: Message, state: State) -> Result<()> {
    tracing::debug!(
        "play command in channel {} by {}",
        msg.channel_id,
        msg.author.name
    );
    // state
    //     .http
    //     .create_message(msg.channel_id)
    //     .content("What's the URL of the audio to play?")?
    //     .exec()
    //     .await?;

    // let author_id = msg.author.id;
    // let msg = state
    //     .standby
    //     .wait_for_message(msg.channel_id, move |new_msg: &MessageCreate| {
    //         new_msg.author.id == author_id
    //     })
    //     .await?;

    let guild_id = msg.guild_id.unwrap();

    if let Ok(song) = Restartable::ytdl(msg.content.clone(), false).await {
        let input = Input::from(song);

        let content = format!(
            "Playing **{:?}** by **{:?}**",
            input
                .metadata
                .track
                .as_ref()
                .unwrap_or(&"<UNKNOWN>".to_string()),
            input
                .metadata
                .artist
                .as_ref()
                .unwrap_or(&"<UNKNOWN>".to_string()),
        );

        state
            .http
            .create_message(msg.channel_id)
            .content(&content)?
            .exec()
            .await?;

        if let Some(call_lock) = state.songbird.get(guild_id) {
            let mut call = call_lock.lock().await;
            let _handle = call.play_source(input);
        }
    } else {
        state
            .http
            .create_message(msg.channel_id)
            .content("Didn't find any results")?
            .exec()
            .await?;
    }

    Ok(())
}
