use futures_util::StreamExt;
use songbird::{
    input::{Input, Restartable},
    tracks::{PlayMode, TrackHandle},
    Songbird,
};
use std::{collections::HashMap, env, error::Error, future::Future, sync::Arc};
use tokio::sync::{mpsc, RwLock};
use twilight_gateway::{Cluster, Event, Intents};
use twilight_http::Client as HttpClient;
use twilight_model::{
    channel::Message,
    id::{marker::GuildMarker, Id},
};

type State = Arc<StateRef>;

#[derive(Debug)]
pub struct StateRef {
    http: HttpClient,
    trackdata: RwLock<HashMap<Id<GuildMarker>, TrackHandle>>,
    songbird: Songbird,
}

fn spawn(
    fut: impl Future<Output = Result<(), Box<dyn Error + Send + Sync + 'static>>> + Send + 'static,
    shutdown: mpsc::Sender<Box<dyn Error + Send + Sync + 'static>>,
) {
    tokio::spawn(async move {
        if let Err(why) = fut.await {
            let _ = shutdown.send(why).await;
        }
    });
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
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

        (
            events,
            Arc::new(StateRef {
                http,
                trackdata: Default::default(),
                songbird,
            }),
        )
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

async fn join(state: State) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let (_handle, status) = state
        .songbird
        .join(532298747284291584, 743945715683819661)
        .await;

    if status.is_ok() {
        tracing::info!("Successfully connected to discord channel");
    }

    status.map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync + 'static>)
}

pub async fn leave(
    msg: Message,
    state: State,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
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

pub async fn play(
    msg: Message,
    state: State,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
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
            let handle = call.play_source(input);

            let mut store = state.trackdata.write().await;
            store.insert(guild_id, handle);
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

pub async fn pause(
    msg: Message,
    state: State,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    tracing::debug!(
        "pause command in channel {} by {}",
        msg.channel_id,
        msg.author.name
    );

    let guild_id = msg.guild_id.unwrap();

    let store = state.trackdata.read().await;

    let content = if let Some(handle) = store.get(&guild_id) {
        let info = handle.get_info().await?;

        let paused = match info.playing {
            PlayMode::Play => {
                let _success = handle.pause();
                false
            }
            _ => {
                let _success = handle.play();
                true
            }
        };

        let action = if paused { "Unpaused" } else { "Paused" };

        format!("{} the track", action)
    } else {
        format!("No track to (un)pause!")
    };

    state
        .http
        .create_message(msg.channel_id)
        .content(&content)?
        .exec()
        .await?;

    Ok(())
}

pub async fn stop(
    msg: Message,
    state: State,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    tracing::debug!(
        "stop command in channel {} by {}",
        msg.channel_id,
        msg.author.name
    );

    let guild_id = msg.guild_id.unwrap();

    if let Some(call_lock) = state.songbird.get(guild_id) {
        let mut call = call_lock.lock().await;
        let _ = call.stop();
    }

    state
        .http
        .create_message(msg.channel_id)
        .content("Stopped the track")?
        .exec()
        .await?;

    Ok(())
}
