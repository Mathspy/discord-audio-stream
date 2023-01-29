use anyhow::{Context, Error, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use futures_util::StreamExt;
use ringbuf::{Consumer, HeapRb};
use songbird::{
    input::{reader::MediaSource, Input},
    Songbird,
};
use std::{env, future::Future, io, sync::Arc};
use tokio::sync::mpsc;
use twilight_gateway::{Cluster, Event, Intents};
use twilight_http::Client as HttpClient;
use twilight_model::channel::Message;

type State = Arc<StateRef>;

const LATENCY: f32 = 500.0;

struct InputStream {
    buffer: Consumer<u8, Arc<HeapRb<u8>>>,
}

impl io::Read for InputStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.buffer.read(buf)
    }
}

impl io::Seek for InputStream {
    fn seek(&mut self, _: io::SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Other,
            "source does not support seeking",
        ))
    }
}

impl MediaSource for InputStream {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        None
    }
}

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
    let (handle, status) = state
        .songbird
        .join(1038578007759147041, 1038578008879005700)
        .await;

    status.context("Failed to join channel")?;

    tracing::info!("Successfully connected to discord channel");
    tracing::info!("Creating audio stream");

    let host = cpal::default_host();
    let default_input_device = host
        .input_devices()?
        .find(|device| {
            device
                .name()
                .map(|name| name == "Game Capture HD60 X")
                .unwrap_or(false)
        })
        .expect("game capture input device exists");
    tracing::info!(
        "Using default input device {:?}",
        default_input_device.name()
    );
    let default_config = default_input_device
        .default_input_config()
        .expect("a default input configuration")
        .config();

    let latency_frames = (LATENCY / 1_000.0) * default_config.sample_rate.0 as f32;
    let latency_samples = latency_frames as usize * default_config.channels as usize * 4;
    let (mut producer, consumer) = HeapRb::<u8>::new(latency_samples * 2).split();

    // Fill the samples with 0 equal to the length of the delay.
    for _ in 0..latency_samples {
        // The ring buffer has twice as much space as necessary to add latency here,
        // so this should never fail
        producer.push(0).unwrap();
    }

    let is_stereo = match default_config.channels {
        1 => false,
        2 => true,
        channels => anyhow::bail!("Input device has an unsupported number of channels: {channels}"),
    };

    tokio::task::spawn_blocking(move || {
        let stream = default_input_device
            .build_input_stream(
                &default_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let data = bytemuck::cast_slice(data);
                    if producer.push_slice(data) != data.len() {
                        tracing::error!("stream fell behind :<")
                    }
                },
                |err| {
                    tracing::error!("an error occurred while streaming audio: {}", err);
                },
                None,
            )
            .expect("Failed to build input stream");
        stream.play().expect("Failed to start input stream");
        tracing::info!("Audio stream started");

        loop {
            std::thread::park();
        }
    });

    let input = Input::float_pcm(
        is_stereo,
        songbird::input::Reader::Extension(Box::new(InputStream { buffer: consumer })),
    );

    let mut call = handle.lock().await;

    call.play_only_source(input);

    Ok(())
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
