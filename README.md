# Discord Audio Stream

Stream any input device from your computer right to a Discord bot.

## Why?

I wanted to be able to show friends things via a video capture device connected
to my laptop on Discord. That's fairly straightforward but the audio doesn't
get streamed.\
There's no simple builtin solution in Discord for this. So... I made my own.

The main difference between this and [mixing your audio capture and microphone](https://github.com/Mathspy/loopback-clone)
together is that it allows people to adjust the volumes or mute the audio
stream individually without telling you to and while picking different
preferences.

## How?

You will need to create a Discord bot and get its token, the bot only needs two
permissions: Voice Connect and Voice Speak. It also needs to be able to see the
voice channel you want it to join.
After that set the bot's token into an enviromental variable `DISCORD_TOKEN`
and run `discord-audio-stream`!

```sh
Discord bot to stream an input device from your computer right into a Discord channel

Usage: discord-audio-stream [OPTIONS] <GUILD_ID> <CHANNEL_ID>

Arguments:
  <GUILD_ID>    id of the server containing the channel to join
  <CHANNEL_ID>  id of the channel to join

Options:
  -i, --input-device <INPUT_DEVICE>  the name of the input_device to stream from
  -l, --latency <LATENCY>            the latency of the audio stream in milliseconds [default: 500]
  -h, --help                         Print help
  -V, --version                      Print version
```

The guild/server id is required and so is the id of the channel in the server
to connect to. A specific input device is optional with the default being...
well the default input device of the computer. The latency is optional
and defaults to a cautious 500ms. Be careful while using with your laptop
microphone if you're in the same channel as the bot due to feedback.

To shut down the bot and disconnect it from the channel, Ctrl+C (or send a
SIGINT) to the process.

## Aside

This was really fun to make, it and [loopback-clone](https://github.com/Mathspy/loopback-clone)
were my first foray into audio and honestly it went great! I learnt a lot and
made something actually useful to me and others. Thanks to my friend Tessa for
helping me demystify some audio concepts at the beginning of my adventures and
the [YouTuber Jocelyn Stericker](https://www.youtube.com/watch?v=ZweInbMBsa4)
for making me realize audio is not really as complicated as I thought.

## License

This project is licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or
   http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or
   http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this project by you, as defined in the Apache-2.0 license,
shall be dual licensed as above, without any additional terms or conditions.
