use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use futures_util::StreamExt;
use ringbuf::{Consumer, HeapRb};
use songbird::{
    input::{reader::MediaSource, Input},
    Songbird,
};
use std::{env, io, sync::Arc};
use tokio::sync::broadcast;
use twilight_gateway::{Cluster, Event, Intents};
use twilight_http::Client as HttpClient;

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

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the tracing subscriber.
    tracing_subscriber::fmt::init();

    let (mut events, songbird) = {
        let token = env::var("DISCORD_TOKEN")?;

        let http = HttpClient::new(token.clone());
        let user_id = http.current_user().exec().await?.model().await?.id;

        let intents = Intents::GUILD_VOICE_STATES;
        let (cluster, events) = Cluster::new(token, intents).await?;
        cluster.up().await;

        let songbird = Songbird::twilight(Arc::new(cluster), user_id);

        (events, Arc::new(songbird))
    };

    let (signal_tx, signal_rx) = broadcast::channel::<Signal>(2);

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                signal_tx.send(Signal::Shutdown)?;
                songbird.leave(532298747284291584).await?;

                return Ok(());
            }
            Some((_, event)) = events.next() => {
                songbird.process(&event).await;

                if let Event::Ready(_) = event {
                    tokio::spawn(join(Arc::clone(&songbird), signal_rx.resubscribe()))
                        .await
                        .context("join task failed unexpectedly")?
                        .context("failed to join channel")?;
                }

            }
        }
    }
}

#[derive(Debug, Clone)]
enum Signal {
    Shutdown,
}

async fn join(
    songbird: Arc<Songbird>,
    mut signal_channel: broadcast::Receiver<Signal>,
) -> Result<()> {
    let (handle, status) = songbird.join(532298747284291584, 743945715683819661).await;

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
            let signal = signal_channel.try_recv();
            match signal {
                Ok(Signal::Shutdown) | Err(broadcast::error::TryRecvError::Closed) => {
                    drop(stream);
                    break;
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Empty) => std::thread::yield_now(),
            }
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
