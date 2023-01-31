use anyhow::{Context, Result};
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use futures_util::{StreamExt, TryStreamExt};
use ringbuf::{Consumer, HeapRb};
use songbird::{
    input::{reader::MediaSource, Input},
    Songbird,
};
use std::{env, io, sync::Arc};
use tokio::{runtime::Handle, sync::watch};
use twilight_gateway::{Cluster, Event, Intents};
use twilight_http::Client as HttpClient;

/// Discord bot to stream an input device from your computer right into a Discord channel
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// id of the server containing the channel to join
    guild_id: u64,

    /// id of the channel to join
    channel_id: u64,

    /// the name of the input_device to stream from
    #[arg(short, long)]
    input_device: Option<String>,

    /// the latency of the audio stream in milliseconds
    #[arg(short, long, default_value_t = 500.0)]
    latency: f32,
}

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

    let args = Args::parse();

    let (events, songbird) = {
        let token = env::var("DISCORD_TOKEN")?;

        let http = HttpClient::new(token.clone());
        let user_id = http.current_user().exec().await?.model().await?.id;

        let intents = Intents::GUILD_VOICE_STATES;
        let (cluster, events) = Cluster::new(token, intents).await?;
        cluster.up().await;

        let songbird = Songbird::twilight(Arc::new(cluster), user_id);

        (events, Arc::new(songbird))
    };

    let config = Arc::new(args);
    let (signal_tx, signal_rx) = watch::channel(Signal::Start);

    let event_stream = events
        .then(futures_util::future::ok)
        .try_for_each_concurrent(None, |(_, event)| async {
            let songbird = Arc::clone(&songbird);
            let config = Arc::clone(&config);
            let signal_rx = signal_rx.clone();

            tokio::spawn(async move {
                songbird.process(&event).await;

                if let Event::Ready(_) = event {
                    join(songbird, config, signal_rx)
                        .await
                        .context("failed to join channel")?;
                }

                Ok(())
            })
            .await?
        });

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            signal_tx.send(Signal::Shutdown)?;
            songbird.leave(config.guild_id).await?;

            return Ok(());
        }
        result = event_stream => {
            return result;
        }
    }
}

#[derive(Debug, Clone)]
enum Signal {
    Start,
    Shutdown,
}

async fn join(
    songbird: Arc<Songbird>,
    config: Arc<Args>,
    mut signal_channel: watch::Receiver<Signal>,
) -> Result<()> {
    tracing::info!("Inspecting input device...");

    let host = cpal::default_host();
    let input_device = if let Some(input_device) = &config.input_device {
        host.input_devices()?
            .find(|device| {
                device
                    .name()
                    .map(|name| &name == input_device)
                    .unwrap_or(false)
            })
            .with_context(|| {
                let input_devices = host
                    .input_devices()
                    .expect("method just succeeded earlier")
                    .flat_map(|device| device.name())
                    .reduce(|mut acc, name| {
                        acc.push_str(", ");
                        acc.push_str(&name);

                        acc
                    });
                if let Some(input_devices) = input_devices {
                    format!(
                        "Unknown input device, please choose one from the available input devices: {}",
                        input_devices
                    )
                } else {
                    format!("Found no input devices, please make sure your micrphone or game capture is connected")
                }
            })?
    } else {
        host.default_input_device().context(
            "No default input device exists and no input device provided via --input-device flag",
        )?
    };
    tracing::info!("Using input device {:?}", input_device.name());
    let default_config = input_device
        .default_input_config()
        .expect("a default input configuration")
        .config();

    let latency_frames = (config.latency / 1_000.0) * default_config.sample_rate.0 as f32;
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

    let (handle, status) = songbird.join(config.guild_id, config.channel_id).await;

    status.context("Failed to join channel")?;

    tracing::info!("Successfully connected to discord channel");
    tracing::info!("Creating audio stream");

    tokio::task::spawn_blocking(move || {
        let stream = input_device
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

        loop {
            let sender_dropped = Handle::current()
                .block_on(signal_channel.changed())
                .is_err();
            if sender_dropped {
                drop(stream);
                break;
            }
            match *signal_channel.borrow() {
                Signal::Start => {
                    stream.play().expect("Failed to start input stream");
                    tracing::info!("Audio stream started");
                }
                Signal::Shutdown => {
                    drop(stream);
                    break;
                }
            }
        }
    });

    let input = Input::float_pcm(
        is_stereo,
        songbird::input::Reader::Extension(Box::new(InputStream { buffer: consumer })),
    );

    let mut call = handle.lock().await;

    tracing::info!("Playing audio for the masses!");
    call.play_only_source(input);

    Ok(())
}
