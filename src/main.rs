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
// use tokio::time;
use tracing::{error, info};

use anyhow::Result;

use std::time::{Duration, Instant};
use std::{collections::HashMap, env, sync::Arc};
type Error = Box<dyn std::error::Error + Send + Sync>;
type Contx<'a> = poise::Context<'a, Data, Error>;
const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes
const ALONE_TIMEOUT: Duration = Duration::from_secs(120); // 2 minutes when alone
const INACTIVITY: &str = "Inactivity";
const BOT_ALONE: &str = "Bot is alone";
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
    async fn start_timer(
        &self,
        ctx: Context,
        guild_id: GuildId,
        duration: Duration,
        timer_name: &str,
    ) {
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
        let timer_name_clone = timer_name.to_owned().clone();
        // let ctx_clone = ctx.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(duration).await;

            if let Some(manager) = songbird::get(&ctx_ser_clone).await {
                if let Some(handler_lock) = manager.get(guild_id) {
                    let handler = handler_lock.lock().await;
                    let queue_check = handler.queue().is_empty();

                    if queue_check {
                        drop(handler);
                        let _ = manager.remove(guild_id).await;

                        if let Ok(channels) = ctx_ser_clone.http.get_channels(guild_id.into()).await
                        {
                            for channel in channels {
                                match channel.kind {
                                    serenity::model::channel::ChannelType::Text => {
                                        let _ = channel
                                            .id
                                            .say(
                                                &ctx_ser_clone.http,
                                                format!(
                                                    "👋 Left voice channel due to {} minutes of {}",
                                                    duration.as_secs() / 60,
                                                    timer_name_clone
                                                ),
                                            )
                                            .await;
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
    pub async fn start_inactivity_timer(&self, guild_id: GuildId, ctx: Context, timer_name: &str) {
        self.start_timer(ctx, guild_id, INACTIVITY_TIMEOUT, timer_name)
            .await;
    }

    // Start timer when bot is alone in voice channel
    pub async fn start_alone_timer(&self, guild_id: GuildId, ctx: Context, timer_name: &str) {
        self.start_timer(ctx, guild_id, ALONE_TIMEOUT, timer_name)
            .await;
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

    pub async fn if_bot_alone(&self, ctx: &Context, guild_id: GuildId) -> bool {
        if let Some(manager) = songbird::get(ctx).await {
            if let Some(handler_lock) = manager.get(guild_id) {
                let handler = handler_lock.lock().await;

                if let Some(current_channel) = handler.current_channel() {
                    let channel_id = ChannelId::from(current_channel.0);

                    let bot_user_id = ctx.cache.current_user().id;
                    if let Some(guild) = ctx.cache.guild(guild_id) {
                        let human_count = guild
                            .voice_states
                            .values()
                            .filter(|vs| vs.channel_id == Some(channel_id))
                            .filter(|vs| vs.user_id != bot_user_id)
                            .filter_map(|vs| guild.members.get(&vs.user_id))
                            .filter(|member| !member.user.bot)
                            .count();

                        info!(
                            "Found {} humans in voice channel {}",
                            human_count, channel_id
                        );
                        return human_count == 0;
                    } else {
                        info!("Guild {} not in cache, assuming not alone", guild_id);
                        return false; // If guild not in cache, assume not alone
                    }
                }
            }
        }
        false
    }

    pub async fn handle_channel_state_change(&self, guild_id: GuildId, ctx: &Context) {
        info!(
            "🔄 handle_channel_state_change called for guild {}",
            guild_id
        );

        let is_alone = self.if_bot_alone(ctx, guild_id).await;

        if is_alone {
            info!("   🎯 Bot is alone, switching to 2-minute timer");
            self.switch_to_alone_timer(guild_id, ctx.clone()).await;
        } else {
            info!("   🎯 Bot is not alone, switching to 5-minute timer");
            self.switch_to_inactivity_timer(guild_id, ctx.clone()).await;
        }

        info!(
            "✅ handle_channel_state_change completed for guild {}",
            guild_id
        );
    }

    pub async fn switch_to_alone_timer(&self, guild_id: GuildId, ctx: Context) {
        info!(
            "⏰ Switching to 2-minute alone timer for guild {}",
            guild_id
        );

        // Cancel any existing timer first
        self.cancel_timers(guild_id).await;

        // Start new 2-minute timer
        self.start_alone_timer(guild_id, ctx, BOT_ALONE).await;

        info!("✅ 2-minute timer started for guild {}", guild_id);
    }

    pub async fn switch_to_inactivity_timer(&self, guild_id: GuildId, ctx: Context) {
        info!(
            "⏰ Switching to 5-minute inactivity timer for guild {}",
            guild_id
        );

        // Cancel any existing timer first
        self.cancel_timers(guild_id).await;

        // Start new 5-minute timer
        self.start_inactivity_timer(guild_id, ctx, INACTIVITY).await;

        info!("✅ 5-minute timer started for guild {}", guild_id);
    }

    // Updated handle_voice_join for initial join
    pub async fn handle_voice_join(&self, guild_id: GuildId, ctx: &Context) {
        info!("Bot joined voice channel in guild {}", guild_id);

        // Update activity timestamp
        self.update_activity(guild_id).await;

        // Determine initial timer based on channel state
        self.handle_channel_state_change(guild_id, ctx).await;
    }

    pub async fn handle_music_activity(&self, guild_id: GuildId) {
        info!(
            "Music activity detected in guild {}, cancelling auto-disconnect timers",
            guild_id
        );

        // Cancel any running timers since music is now playing
        self.cancel_timers(guild_id).await;

        // Update activity timestamp
        self.update_activity(guild_id).await;
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

    async fn voice_state_update(&self, ctx: Context, old: Option<VoiceState>, new: VoiceState) {
        info!("🔄 Voice state update received:");
        info!("   User: {} ({})", new.user_id, new.user_id);
        info!("   Guild: {:?}", new.guild_id);
        info!(
            "   Old channel: {:?}",
            old.as_ref().and_then(|o| o.channel_id)
        );
        info!("   New channel: {:?}", new.channel_id);
        let Some(guild_id) = new.guild_id else { return };

        // Check if our bot is in a voice channel in this guild
        if let Some(manager) = songbird::get(&ctx).await {
            if let Some(handler_lock) = manager.get(guild_id) {
                let handler = handler_lock.lock().await;

                if let Some(current_channel) = handler.current_channel() {
                    let bot_channel_id = ChannelId::from(current_channel.0);

                    // Check if the voice state change affects our bot's channel
                    let old_affects_bot = old
                        .as_ref()
                        .map(|o| o.channel_id == Some(bot_channel_id))
                        .unwrap_or(false);
                    let new_affects_bot = new.channel_id == Some(bot_channel_id);
                    let affects_bot_channel = old_affects_bot || new_affects_bot;

                    if affects_bot_channel {
                        info!(
                            "Voice state change affects bot's channel {} in guild {}",
                            bot_channel_id, guild_id
                        );

                        // Don't process our own bot's voice state changes
                        let bot_user_id = ctx.cache.current_user().id;
                        if (new.user_id) == bot_user_id {
                            return;
                        }

                        let queue_empty = handler.queue().is_empty();
                        drop(handler);

                        if queue_empty {
                            tokio::time::sleep(Duration::from_millis(500)).await;

                            self.auto_disconnect
                                .handle_channel_state_change(guild_id, &ctx)
                                .await;
                        } else {
                            info!("   🎵 Music is playing, not managing timers");
                        }
                    } else {
                        info!("   ❌ Voice state change doesn't affect bot's channel");
                    }
                } else {
                    info!("   ❌ Bot is not in any voice channel in this guild");
                }
            }
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
                        .start_inactivity_timer(self.guild_id, self.ctx.clone(), INACTIVITY)
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
    ctx.data()
        .auto_disconnect
        .handle_music_activity(guild_id)
        .await;

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
            .start_inactivity_timer(guild_id, ctx.serenity_context().clone(), INACTIVITY)
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
    tracing_subscriber::fmt().init();
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
        .event_handler(BotEventHandler::new(auto_disconnect.clone()))
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
