use dotenv::dotenv;
use poise::serenity_prelude::GatewayIntents;
use reqwest::Client as HttpClient;
use serenity::{model::user, prelude::TypeMapKey};
use songbird::{
    SerenityInit,
    input::{Compose, HttpRequest, YoutubeDl},
};
use tracing::{error, info};

use std::env;
use tracing::warn;
struct Handler;
use anyhow::Result;
use serenity::client::Client;
use std::time::Duration;
use tokio::process::Command;
pub struct Data {}
type Error = Box<dyn std::error::Error + Send + Sync>;
type Contx<'a> = poise::Context<'a, Data, Error>;
#[poise::command(prefix_command, slash_command)]
async fn help(
    ctx: Contx<'_>,
    #[description = "Specific command to show help about"]
    #[autocomplete = "poise::builtins::autocomplete_command"]
    command: Option<String>,
) -> Result<(), Error> {
    poise::builtins::help(
        ctx,
        command.as_deref(),
        poise::builtins::HelpConfiguration {
            extra_text_at_bottom: "🎵 Discord Music Bot - Play music from YouTube in voice channels!",
            ..Default::default()
        },
    ).await?;
    Ok(())
}

struct YoutubeSearch;

impl YoutubeSearch {
    pub async fn search_url(query: &str) -> Result<String> {
        Ok(format!("ytsearch:{}", query))
    }
}

struct HttpKey;

impl TypeMapKey for HttpKey {
    type Value = HttpClient;
}
async fn extract_direct_audio_url(query: &str) -> anyhow::Result<String> {
    // Prefer opus/webm when present, otherwise any best audio
    let args = [
        "--no-config",
        "--no-playlist",
        "-g",
        "-f",
        "ba/b",
        "--format-sort",
        "acodec:opus,ext:webm",
        query,
    ];

    // Run the same thing that works in your shell
    let out = Command::new("yt-dlp").args(args).output().await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("yt-dlp failed: {err}");
    }

    // First line is the direct audio URL
    let url = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("yt-dlp returned no URL"))?
        .trim()
        .to_string();

    Ok(url)
}
fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let hours = minutes / 60;

    if hours > 0 {
        format!("{}:{:02}:{:02}", hours, minutes % 60, seconds % 60)
    } else {
        format!("{}:{:02}", minutes, seconds % 60)
    }
}

/// Join your voice channel
#[poise::command(slash_command, guild_only)]
async fn join(ctx: Contx<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().ok_or("Not in a guild")?;

    // Extract the data you need from guild BEFORE any await points
    let user_channel_id = {
        let guild = ctx.guild().ok_or("Guild not found")?;
        guild
            .voice_states
            .get(&ctx.author().id)
            .and_then(|voice_state| voice_state.channel_id)
    };
    let connect_to = match user_channel_id {
        Some(channel) => channel,
        None => {
            ctx.say("❌ You need to be in a voice channel!").await?;
            return Ok(());
        }
    };

    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird not initialized")?;

    match manager.join(guild_id, connect_to).await {
        Ok(handler_lock) => {
            let mut handler = handler_lock.lock().await;
            handler.deafen(true).await?;

            ctx.say(format!("✅ Joined <#{}>!", connect_to)).await?;
        }
        Err(e) => {
            // error!("Error joining channel: {:?}", e);
            ctx.say("❌ Error joining the channel").await?;
        }
    }

    Ok(())
}

// Play a song from YouTube
#[poise::command(slash_command, guild_only)]
async fn play(
    ctx: Contx<'_>,
    #[description = "Song name or YouTube URL"] query: String,
) -> Result<(), Error> {
    if query.is_empty() {
        ctx.say("❌ Please provide a song name or YouTube URL!")
            .await?;
        return Ok(());
    }
    // let format_pref = "bestaudio[ext=webm]/bestaudio[ext=m4a]/bestaudio/best";

    let guild_id = ctx.guild_id().ok_or("Not in a guild")?;

    // Extract the data you need from guild BEFORE any await points
    let user_channel_id = {
        let guild = ctx.guild().ok_or("Guild not found")?;
        guild
            .voice_states
            .get(&ctx.author().id)
            .and_then(|voice_state| voice_state.channel_id)
    };
    // Now we can safely use await points
    ctx.defer().await?;

    let manager = songbird::get(ctx.serenity_context())
        .await
        .ok_or("Songbird not initialized")?;

    // Auto-join if not in channel
    if manager.get(guild_id).is_none() {
        match user_channel_id {
            Some(channel) => match manager.join(guild_id, channel).await {
                Ok(handler_lock) => {
                    let mut handler = handler_lock.lock().await;
                    handler.deafen(true).await?;
                }
                Err(e) => {
                    ctx.say(format!("❌ Couldn't join your voice channel: {:?}", e))
                        .await?;
                    return Ok(());
                }
            },
            None => {
                ctx.say("❌ You need to be in a voice channel!").await?;
                return Ok(());
            }
        }
    }

    let handler_lock = manager.get(guild_id).unwrap();

    // Search YouTube if not a URL
    // let processed = if query.starts_with("http") {
    //     query.clone()
    // } else {
    //     format!("ytsearch1:{query}")
    // };
    // let mut audio_url = match extract_direct_audio_url(&processed).await {
    //     Ok(u) => u,
    //     // 2) Fallback: loosen selection if the first attempt fails
    //     Err(_) => {
    //         let out = Command::new("yt-dlp")
    //             .args([
    //                 "--no-config",
    //                 "--no-playlist",
    //                 "-g",
    //                 "-f",
    //                 "ba/b",
    //                 &processed,
    //             ])
    //             .output()
    //             .await?;
    //         if !out.status.success() {
    //             let err = String::from_utf8_lossy(&out.stderr);
    //             ctx.say(format!("❌ yt-dlp failed: {err}")).await?;
    //             return Ok(());
    //         }
    //         String::from_utf8_lossy(&out.stdout)
    //             .lines()
    //             .next()
    //             .unwrap_or("")
    //             .to_string()
    //     }
    // };
    // if audio_url.is_empty() {
    //     ctx.say("❌ Couldn't extract an audio stream for that query.")
    //         .await?;
    //     return Ok(());
    // }
    let client = reqwest::Client::new();

    // let source = YoutubeDl::new(reqwest::Client::new(), processed_query.clone()).user_args(vec![
    //     "--no-playlist".into(), // avoid entire playlists by accident
    //     "-f".into(),
    //     format_pref.into(), // <— key fix: safe fallback chain
    // ]);
    // let source = YoutubeDl::new(client.clone(), processed_query.clone()).user_args(vec![
    //     "--no-config".into(),
    //     "--no-playlist".into(),
    //     "-f".into(),
    //     "ba/b".into(), // the loosest reliable audio selector
    // ]);
    let mut meta_ytdl = if query.starts_with("http") {
        YoutubeDl::new_ytdl_like("yt-dlp", client.clone(), query.clone())
            .user_args(vec!["--no-config".into(), "--no-playlist".into()])
    } else {
        // Use the dedicated *search* constructor for queries.
        YoutubeDl::new_search_ytdl_like("yt-dlp", client.clone(), query.clone())
            .user_args(vec!["--no-config".into(), "--no-playlist".into()])
    };

    let meta = meta_ytdl.aux_metadata().await?;
    let title = meta.title.as_deref().unwrap_or("Unknown");
    let duration_str = meta
        .duration
        .map(format_duration)
        .unwrap_or_else(|| "Unknown".to_string());
    let play_args = vec![
        "--no-config".into(),
        "--no-playlist".into(),
        "-f".into(),
        "ba/b".into(), // bestaudio or best
        "--format-sort".into(),
        "acodec:opus,ext:webm".into(), // prefer Opus/WebM when present
    ];
    let play_ytdl = if query.starts_with("http") {
        YoutubeDl::new_ytdl_like("yt-dlp", client.clone(), query.clone()).user_args(play_args)
    } else {
        YoutubeDl::new_search_ytdl_like("yt-dlp", client.clone(), query.clone())
            .user_args(play_args)
    };
    // Play the track
    let mut handler = handler_lock.lock().await;
    handler.play_input(play_ytdl.into());

    ctx.say(format!(
        "🎵 **Now playing:** {} [{}]\n📝 Requested by: {}",
        title,
        duration_str,
        ctx.author().name
    ))
    .await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Configure the client with your Discord bot token in the environment.
    dotenv().ok();
    let token = env::var("DISCORD_TOKEN").expect("Expected DISCORD_TOKEN in environment");
    println!("{:?}", token);
    let intents = GatewayIntents::non_privileged() | GatewayIntents::GUILD_VOICE_STATES;
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![help(), play(), join()],
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(Data {})
            })
        })
        .build();
    println!("framework done");
    let mut client = Client::builder(token, intents)
        .framework(framework)
        .register_songbird()
        .type_map_insert::<HttpKey>(reqwest::Client::new())
        .await?;
    println!("client initaited");
    tokio::spawn(async move {
        let _ = client
            .start()
            .await
            .map_err(|why| println!("Client ended: {:?}", why));
    });

    let _signal_err = tokio::signal::ctrl_c().await;
    println!("Received Ctrl-C, shutting down.");
    Ok(())
}
