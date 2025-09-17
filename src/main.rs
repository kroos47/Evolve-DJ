use dotenv::dotenv;

use poise::serenity_prelude::GatewayIntents;
use reqwest::Client as HttpClient;
use serenity::async_trait;
use serenity::client::{Client, Context, EventHandler};
use serenity::model::gateway::Ready;
use serenity::model::id::{ChannelId, GuildId};
use serenity::model::voice::VoiceState;
use serenity::prelude::TypeMapKey;
use songbird::{Event, EventContext, EventHandler as SongbirdEventHandler, TrackEvent};
// use songbird::id::ChannelId;

use songbird::{
    SerenityInit,
    input::{Compose, YoutubeDl},
};
use tokio::sync::RwLock;
use tracing::{error, info};

use anyhow::Result;

use std::time::{Duration, Instant};
use std::{collections::HashMap, env, sync::Arc};
type Error = Box<dyn std::error::Error + Send + Sync>;
type Contx<'a> = poise::Context<'a, Data, Error>;
const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes
const ALONE_TIMEOUT: Duration = Duration::from_secs(120); // 2 minutes when alone
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

struct HttpKey;

impl TypeMapKey for HttpKey {
    type Value = HttpClient;
}

#[derive(Clone)]
pub struct Data {
    pub auto_disconnect: Arc<AutoDisconnectManager>,
}

pub struct AutoDisconnectManager {
    guild_timer: RwLock<HashMap<GuildId, GuildTimer>>,
}

pub struct GuildTimer {
    last_activity: Instant,
    timer_task: Option<tokio::task::JoinHandle<()>>,
}

impl AutoDisconnectManager {
    pub fn new() -> Self {
        Self {
            guild_timer: RwLock::new(HashMap::new()),
        }
    }
    async fn start_timer(&self, ctx: Context, guild_id: GuildId, duration: Duration) {
        let mut timers = self.guild_timer.write().await;

        //abort if already existing timer
        if let Some(timer) = timers.get_mut(&guild_id) {
            if let Some(task) = timer.timer_task.take() {
                task.abort();
            }
        } else {
            timers.insert(
                guild_id,
                GuildTimer {
                    last_activity: Instant::now(),
                    timer_task: None,
                },
            );
        }
        let guild_timer = timers.get_mut(&guild_id).unwrap();

        guild_timer.last_activity = Instant::now();

        let ctx_ser_clone = ctx.clone();

        // let ctx_clone = ctx.clone();
        let task =
            tokio::spawn(async move {
                tokio::time::sleep(duration).await;

                if let Some(manager) = songbird::get(&ctx_ser_clone).await {
                    if let Some(handler_lock) = manager.get(guild_id) {
                        let handler = handler_lock.lock().await;
                        let queue_check = handler.queue().is_empty();

                        if queue_check {
                            drop(handler);
                            let _ = manager.remove(guild_id).await;

                            if let Ok(channels) =
                                ctx_ser_clone.http.get_channels(guild_id.into()).await
                            {
                                for channel in channels {
                                    match channel.kind {
                                        serenity::model::channel::ChannelType::Text => {
                                            let _ = channel.id.say(&ctx_ser_clone.http,
                                            format!("🔇 Left voice channel due to {} minutes of {}",
                                                   duration.as_secs() / 60, "timer_name")).await;
                                            break;
                                        }
                                        _ => continue,
                                    }
                                }
                            }
                        }
                    }
                }
            });
        guild_timer.timer_task = Some(task);
    }
    pub async fn update_activity(&self, guild_id: GuildId) {
        let mut timers = self.guild_timer.write().await;

        if let Some(guild_timer) = timers.get_mut(&guild_id) {
            guild_timer.last_activity = Instant::now();

            // Cancel existing timer
            if let Some(task) = guild_timer.timer_task.take() {
                task.abort();
            }
        }

        info!("Updated activity for guild {}", guild_id);
    }

    // Start inactivity timer when queue becomes empty
    pub async fn start_inactivity_timer(&self, guild_id: GuildId, ctx: Context) {
        self.start_timer(ctx, guild_id, INACTIVITY_TIMEOUT).await;
    }

    // Start timer when bot is alone in voice channel
    pub async fn start_alone_timer(&self, guild_id: GuildId, ctx: Context) {
        self.start_timer(ctx, guild_id, ALONE_TIMEOUT).await;
    }

    pub async fn cancel_timers(&self, guild_id: GuildId) {
        let mut timers = self.guild_timer.write().await;
        if let Some(guild_timer) = timers.remove(&guild_id) {
            if let Some(task) = guild_timer.timer_task {
                task.abort();
            }
            info!("Cancelled timers for guild {}", guild_id);
        }
    }
    pub async fn check_if_alone(&self, ctx: &Context, guild_id: GuildId) {
        if let Some(manager) = songbird::get(ctx).await {
            if let Some(handler_lock) = manager.get(guild_id) {
                let handler = handler_lock.lock().await;

                if let Some(current_channel) = handler.current_channel() {
                    let channel_id = ChannelId::from(current_channel.0);

                    // Get current bot's user ID
                    let bot_user_id = ctx.cache.current_user().id;

                    // Count non-bot users in the voice channel
                    let human_count = match ctx.cache.guild(guild_id) {
                        Some(guild) => {
                            guild
                                .voice_states
                                .values()
                                .filter(|vs| vs.channel_id == Some(channel_id))
                                .filter(|vs| vs.user_id != bot_user_id) // Exclude our bot
                                .filter_map(|vs| guild.members.get(&vs.user_id))
                                .filter(|member| !member.user.bot) // Exclude all bots
                                .count()
                        }
                        None => {
                            // If guild not in cache, make HTTP request
                            info!(
                                "Guild {} not in cache, cannot determine voice states reliably",
                                guild_id
                            );
                            return;
                        }
                    };

                    if human_count == 0 {
                        info!(
                            "Bot is alone in voice channel (found {} humans), starting alone timer",
                            human_count
                        );
                        drop(handler);
                        self.start_alone_timer(guild_id, ctx.clone()).await;
                    } else {
                        info!("Found {} human(s) in voice channel, not alone", human_count);
                    }
                }
            }
        }
    }

    pub async fn handle_voice_join(&self, guild_id: GuildId, ctx: &Context) {
        // Update activity first (cancels any existing timers)
        self.update_activity(guild_id).await;

        // Check if alone (starts 2-minute timer if alone)
        self.check_if_alone(ctx, guild_id).await;

        // If not alone, start inactivity timer (5 minutes) since no music is playing
        // Note: check_if_alone will override this with a shorter timer if bot is alone
        self.start_inactivity_timer(guild_id, ctx.clone()).await;
    }
}

pub struct BotEventHandler {
    auto_disconnect: Arc<AutoDisconnectManager>,
}

impl BotEventHandler {
    pub fn new(auto_disconnect: Arc<AutoDisconnectManager>) -> Self {
        Self { auto_disconnect }
    }
}

#[async_trait]
impl EventHandler for BotEventHandler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);
    }

    async fn voice_state_update(&self, ctx: Context, _old: Option<VoiceState>, new: VoiceState) {
        // Check if someone left and if bot might be alone now
        if let Some(guild_id) = new.guild_id {
            // Small delay to let voice states update
            tokio::time::sleep(Duration::from_millis(500)).await;
            self.auto_disconnect.check_if_alone(&ctx, guild_id).await;
        }
    }
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

pub struct MusicEventHandler {
    pub guild_id: GuildId,
    pub auto_disconnect: Arc<AutoDisconnectManager>,
    pub ctx: Context,
}

#[async_trait]
impl SongbirdEventHandler for MusicEventHandler {
    async fn act(&self, _ctx: &EventContext<'_>) -> Option<Event> {
        info!("Track ended in guild {}", self.guild_id);

        // Check if queue is now empty
        if let Some(manager) = songbird::get(&self.ctx).await {
            if let Some(handler_lock) = manager.get(self.guild_id) {
                let handler = handler_lock.lock().await;
                if handler.queue().is_empty() {
                    info!("Queue is empty after track end, starting inactivity timer");
                    drop(handler); // Release lock before async call
                    self.auto_disconnect
                        .start_inactivity_timer(self.guild_id, self.ctx.clone())
                        .await;
                }
            }
        }

        None
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
            handler.add_global_event(
                Event::Track(TrackEvent::End),
                MusicEventHandler {
                    guild_id,
                    auto_disconnect: ctx.data().auto_disconnect.clone(),
                    ctx: ctx.serenity_context().clone(),
                },
            );

            drop(handler); // Release lock

            ctx.data()
                .auto_disconnect
                .handle_voice_join(guild_id, ctx.serenity_context())
                .await;
            ctx.say(format!("✅ Joined <#{}>!", connect_to)).await?;
        }
        Err(e) => {
            error!("Error joining channel: {:?}", e);
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

                    handler.add_global_event(
                        Event::Track(TrackEvent::End),
                        MusicEventHandler {
                            guild_id,
                            auto_disconnect: ctx.data().auto_disconnect.clone(),
                            ctx: ctx.serenity_context().clone(),
                        },
                    );
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

    let client = reqwest::Client::new();

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
    handler.enqueue_input(play_ytdl.into()).await;
    ctx.data().auto_disconnect.update_activity(guild_id).await;

    if handler.queue().len() > 1 {
        info!(
            "🎵 **Track added to queue:** {} [{}]\n📝 Requested by: {}",
            title,
            duration_str,
            ctx.author().name
        );
        ctx.say(format!(
            "🎵 **Track added to queue:** {} [{}]\n📝 Requested by: {}",
            title,
            duration_str,
            ctx.author().name
        ))
        .await?;
    } else {
        info!(
            "🎵 **Now playing:** {} [{}]\n📝 Requested by: {}",
            title,
            duration_str,
            ctx.author().name
        );
        ctx.say(format!(
            "🎵 **Now playing:** {} [{}]\n📝 Requested by: {}",
            title,
            duration_str,
            ctx.author().name
        ))
        .await?;
    }
    Ok(())
}

#[poise::command(slash_command, guild_only)]
async fn stop(ctx: Contx<'_>) -> Result<(), Error> {
    let guild_id = ctx.guild_id().unwrap();

    let manager = songbird::get(ctx.serenity_context()).await.unwrap();
    if let Some(handler_lock) = manager.get(guild_id) {
        let handler = handler_lock.lock().await;
        let queue = handler.queue();
        queue.stop();
        info!("⏹️ Stopped playback and cleared queue");
        drop(handler);
        ctx.data()
            .auto_disconnect
            .start_inactivity_timer(guild_id, ctx.serenity_context().clone())
            .await;

        ctx.say("⏹️ Stopped playback and cleared queue").await?;
    } else {
        ctx.say("Not in a voice channel!").await?;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Configure the client with your Discord bot token in the environment.
    dotenv().ok();
    let token = env::var("DISCORD_TOKEN").expect("Expected DISCORD_TOKEN in environment");
    println!("{:?}", token);
    let auto_disconnect = Arc::new(AutoDisconnectManager::new());
    let auto_disconnect_clone = auto_disconnect.clone();

    let intents = GatewayIntents::non_privileged()
        | GatewayIntents::GUILD_VOICE_STATES
        | GatewayIntents::MESSAGE_CONTENT
        | GatewayIntents::GUILDS;

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![help(), join(), play(), stop()],
            ..Default::default()
        })
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(Data {
                    auto_disconnect: auto_disconnect_clone,
                })
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
