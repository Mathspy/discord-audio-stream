use songbird::{
    input::{Input, Restartable},
    tracks::{PlayMode, TrackHandle},
    Songbird,
};
use std::{collections::HashMap, env, error::Error, future::Future, sync::Arc};
use tokio::sync::RwLock;
use twilight_gateway::{Cluster, Intents};
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

pub fn spawn(
    fut: impl Future<Output = Result<(), Box<dyn Error + Send + Sync + 'static>>> + Send + 'static,
) {
    tokio::spawn(async move {
        if let Err(why) = fut.await {
            tracing::debug!("handler error: {:?}", why);
        }
    });
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    // Initialize the tracing subscriber.
    tracing_subscriber::fmt::init();

    let (_events, _state) = {
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

    // while let Some((_, event)) = events.next().await {
    //     state.standby.process(&event);
    //     state.songbird.process(&event).await;

    //     if let Event::MessageCreate(msg) = event {
    //         if msg.guild_id.is_none() || !msg.content.starts_with('!') {
    //             continue;
    //         }

    //         match msg.content.splitn(2, ' ').next() {
    //             Some("!join") => spawn(join(msg.0, Arc::clone(&state))),
    //             Some("!leave") => spawn(leave(msg.0, Arc::clone(&state))),
    //             Some("!pause") => spawn(pause(msg.0, Arc::clone(&state))),
    //             Some("!play") => spawn(play(msg.0, Arc::clone(&state))),
    //             Some("!seek") => spawn(seek(msg.0, Arc::clone(&state))),
    //             Some("!stop") => spawn(stop(msg.0, Arc::clone(&state))),
    //             Some("!volume") => spawn(volume(msg.0, Arc::clone(&state))),
    //             _ => continue,
    //         }
    //     }
    // }

    Ok(())
}

pub async fn join(
    msg: Message,
    state: State,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let channel_id = msg.content.parse::<u64>()?;

    let guild_id = msg.guild_id.ok_or("Can't join a non-guild channel.")?;

    let (_handle, success) = state.songbird.join(guild_id, channel_id).await;

    let content = match success {
        Ok(()) => format!("Joined <#{}>!", channel_id),
        Err(e) => format!("Failed to join <#{}>! Why: {:?}", channel_id, e),
    };

    state
        .http
        .create_message(msg.channel_id)
        .content(&content)?
        .exec()
        .await?;

    Ok(())
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
