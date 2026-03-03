#![allow(clippy::disallowed_macros)]

mod module_bindings;
use module_bindings::*;

use std::env;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use spacetimedb_sdk::{DbContext, Error, Event, Identity, Table, TableWithPrimaryKey, credentials};

// --- Shared state ---

static READY: AtomicBool = AtomicBool::new(false);
static CURRENT_SERVER_ID: AtomicU64 = AtomicU64::new(0);
static CURRENT_CHANNEL_ID: AtomicU64 = AtomicU64::new(0);

fn current_server() -> Option<u64> {
    let v = CURRENT_SERVER_ID.load(Ordering::Relaxed);
    if v == 0 { None } else { Some(v) }
}

fn current_channel() -> Option<u64> {
    let v = CURRENT_CHANNEL_ID.load(Ordering::Relaxed);
    if v == 0 { None } else { Some(v) }
}

// --- Main ---

fn main() {
    let ctx = connect_to_db();
    register_callbacks(&ctx);
    subscribe_to_tables(&ctx);
    ctx.run_threaded();
    user_input_loop(&ctx);
}

// --- Connection ---

fn connect_to_db() -> DbConnection {
    let host = env::var("SPACETIMEDB_HOST")
        .unwrap_or_else(|_| "https://maincloud.spacetimedb.com".to_string());
    let db_name = env::var("SPACETIMEDB_DB_NAME").unwrap_or_else(|_| "spaces".to_string());

    DbConnection::builder()
        .on_connect(on_connected)
        .on_connect_error(on_connect_error)
        .on_disconnect(on_disconnected)
        .with_token(creds_store().load().ok().flatten())
        .with_database_name(db_name)
        .with_uri(host)
        .build()
        .expect("Failed to connect")
}

fn creds_store() -> credentials::File {
    credentials::File::new("spaces")
}

fn on_connected(_ctx: &DbConnection, identity: Identity, token: &str) {
    if let Err(e) = creds_store().save(token) {
        eprintln!("Failed to save credentials: {e:?}");
    }
    println!("Connected as: {}", identity.to_abbreviated_hex());
}

fn on_connect_error(_ctx: &ErrorContext, err: Error) {
    eprintln!("Connection error: {err}");
    std::process::exit(1);
}

fn on_disconnected(_ctx: &ErrorContext, err: Option<Error>) {
    if let Some(err) = err {
        eprintln!("Disconnected: {err}");
        std::process::exit(1);
    } else {
        println!("Disconnected.");
        std::process::exit(0);
    }
}

// --- Callbacks ---

fn register_callbacks(ctx: &DbConnection) {
    ctx.db.user_profile().on_insert(on_user_profile_inserted);
    ctx.db.user_presence().on_update(on_user_presence_updated);
    ctx.db.message().on_insert(on_message_inserted);
    ctx.db.server().on_insert(on_server_inserted);
    ctx.db.channel().on_insert(on_channel_inserted);
}

fn on_user_profile_inserted(ctx: &EventContext, profile: &UserProfile) {
    if matches!(ctx.event, Event::SubscribeApplied) {
        return;
    }
    println!("[+] New user: {}", profile.username);
}

fn on_user_presence_updated(_ctx: &EventContext, old: &UserPresence, new: &UserPresence) {
    let name = _ctx
        .db
        .user_profile()
        .identity()
        .find(&new.identity)
        .map(|p| p.username.clone())
        .unwrap_or_else(|| "unknown".to_string());
    if !old.online && new.online {
        println!("[+] {} came online", name);
    } else if old.online && !new.online {
        println!("[-] {} went offline", name);
    }
}

fn on_message_inserted(ctx: &EventContext, msg: &Message) {
    if matches!(ctx.event, Event::SubscribeApplied) {
        return;
    }
    // Only show messages for the current channel
    if let Some(ch_id) = current_channel()
        && msg.channel_id == ch_id
    {
        print_message(ctx, msg);
    }
}

fn on_server_inserted(ctx: &EventContext, server: &Server) {
    if matches!(ctx.event, Event::SubscribeApplied) {
        return;
    }
    println!("[+] New server: {} (id={})", server.name, server.id);
}

fn on_channel_inserted(ctx: &EventContext, channel: &Channel) {
    if matches!(ctx.event, Event::SubscribeApplied) {
        return;
    }
    println!("[+] New channel: #{} (id={})", channel.name, channel.id);
}

// --- Helpers ---

fn user_display_name(ctx: &impl RemoteDbContext, identity: &Identity) -> String {
    ctx.db()
        .user_profile()
        .identity()
        .find(identity)
        .map(|p| p.display_name.clone().unwrap_or_else(|| p.username.clone()))
        .unwrap_or_else(|| identity.to_abbreviated_hex().to_string())
}

fn print_message(ctx: &impl RemoteDbContext, msg: &Message) {
    let sender = user_display_name(ctx, &msg.sender);
    let type_prefix = match msg.message_type {
        MessageType::System => "[SYSTEM] ",
        MessageType::Join => "[JOIN] ",
        MessageType::Leave => "[LEAVE] ",
        MessageType::AgentEvent => "[AGENT] ",
        MessageType::Text => "",
    };
    let thread_suffix = msg
        .thread_id
        .map(|t| format!(" (thread:{})", t))
        .unwrap_or_default();
    let reply_suffix = msg
        .reply_to_id
        .map(|r| format!(" (reply to:{})", r))
        .unwrap_or_default();
    let edit_mark = if msg.edited_at.is_some() {
        " (edited)"
    } else {
        ""
    };
    println!(
        "{}{}: {}{}{}{}",
        type_prefix, sender, msg.content, thread_suffix, reply_suffix, edit_mark
    );
}

// --- Subscriptions ---

fn subscribe_to_tables(ctx: &DbConnection) {
    ctx.subscription_builder()
        .on_applied(on_sub_applied)
        .on_error(on_sub_error)
        .subscribe_to_all_tables();
}

fn on_sub_applied(ctx: &SubscriptionEventContext) {
    println!("\n=== Spaces - Discord Clone on SpacetimeDB ===\n");

    // List servers
    let servers: Vec<_> = ctx.db.server().iter().collect();
    if servers.is_empty() {
        println!("No servers found.");
    } else {
        println!("Servers:");
        for s in &servers {
            println!("  [{}] {}", s.id, s.name);
        }
    }

    // Auto-select first server
    if let Some(first) = servers.first() {
        CURRENT_SERVER_ID.store(first.id, Ordering::Relaxed);
        println!("\nActive server: {} (id={})", first.name, first.id);

        // Show channels
        let channels: Vec<_> = ctx
            .db
            .channel()
            .iter()
            .filter(|c| c.server_id == first.id)
            .collect();
        if !channels.is_empty() {
            println!("Channels:");
            for ch in &channels {
                println!("  [{}] #{} ({:?})", ch.id, ch.name, ch.channel_type);
            }
            // Auto-select first text channel
            if let Some(text_ch) = channels
                .iter()
                .find(|c| c.channel_type == ChannelType::Text)
            {
                CURRENT_CHANNEL_ID.store(text_ch.id, Ordering::Relaxed);
                println!("\nActive channel: #{} (id={})", text_ch.name, text_ch.id);

                // Show recent messages
                let mut msgs: Vec<_> = ctx
                    .db
                    .message()
                    .iter()
                    .filter(|m| m.channel_id == text_ch.id && m.thread_id.is_none())
                    .collect();
                msgs.sort_by_key(|m| m.created_at);
                let recent: Vec<_> = msgs
                    .iter()
                    .rev()
                    .take(25)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                if !recent.is_empty() {
                    println!("\n--- Recent messages ---");
                    for msg in recent {
                        print_message(ctx, msg);
                    }
                    println!("--- End ---\n");
                }
            }
        }
    }

    println!("Type /help for commands, or type a message to send.");
    println!();
    READY.store(true, Ordering::Release);
}

fn on_sub_error(_ctx: &ErrorContext, err: Error) {
    eprintln!("Subscription failed: {err}");
    std::process::exit(1);
}

// --- User Input ---

fn user_input_loop(ctx: &DbConnection) {
    // Wait for subscription to be applied before processing input
    while !READY.load(Ordering::Acquire) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for line in std::io::stdin().lines() {
        let Ok(line) = line else {
            break;
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('/') {
            handle_command(ctx, &line);
        } else {
            // Send message to current channel
            if let Some(ch_id) = current_channel() {
                if let Err(e) = ctx.reducers.send_message(ch_id, line, None, None) {
                    eprintln!("Failed to send: {e}");
                }
            } else {
                eprintln!("No channel selected. Use /channel <id> to select one.");
            }
        }
    }
}

fn handle_command(ctx: &DbConnection, input: &str) {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match cmd {
        "/help" => print_help(),
        "/name" => cmd_set_name(ctx, arg),
        "/servers" => cmd_list_servers(ctx),
        "/server" => cmd_select_server(ctx, arg),
        "/join" => cmd_join_server(ctx, arg),
        "/leave" => cmd_leave_server(ctx, arg),
        "/create-server" => cmd_create_server(ctx, arg),
        "/channels" => cmd_list_channels(ctx),
        "/channel" => cmd_select_channel(ctx, arg),
        "/create-channel" => cmd_create_channel(ctx, arg),
        "/threads" => cmd_list_threads(ctx),
        "/thread" => cmd_create_thread(ctx, arg),
        "/members" => cmd_list_members(ctx),
        "/react" => cmd_react(ctx, arg),
        "/agent" => cmd_register_agent(ctx, arg),
        "/who" => cmd_who(ctx),
        "/history" => cmd_history(ctx, arg),
        _ => eprintln!("Unknown command: {cmd}. Type /help for available commands."),
    }
}

fn print_help() {
    println!(
        r#"
=== Spaces Commands ===

Navigation:
  /servers                 - List all servers
  /server <id>             - Select active server
  /channels                - List channels in current server
  /channel <id>            - Select active channel
  /threads                 - List threads in current channel
  /history [n]             - Show last n messages (default 25)

Server Management:
  /create-server <name>    - Create a new server
  /join <server_id>        - Join a server
  /leave <server_id>       - Leave a server

Channel Management:
  /create-channel <name>   - Create text channel in current server

Thread Management:
  /thread <name> | <msg>   - Create thread with initial message

Messaging:
  <any text>               - Send message to current channel
  /react <msg_id> <emoji>  - React to a message

User:
  /name <username>         - Set your username
  /agent <name> | <desc>   - Register as an agent
  /who                     - Show online users
  /members                 - List server members
  /help                    - Show this help
"#
    );
}

fn cmd_set_name(ctx: &DbConnection, arg: &str) {
    if arg.is_empty() {
        eprintln!("Usage: /name <username>");
        return;
    }
    if let Err(e) = ctx.reducers.set_profile(arg.to_string(), None, None, None) {
        eprintln!("Failed to set profile: {e}");
    } else {
        println!("Username set to: {}", arg);
    }
}

fn cmd_list_servers(ctx: &DbConnection) {
    let servers: Vec<_> = ctx.db.server().iter().collect();
    if servers.is_empty() {
        println!("No servers found.");
        return;
    }
    println!("Servers:");
    for s in &servers {
        let marker = if current_server() == Some(s.id) {
            " *"
        } else {
            ""
        };
        println!("  [{}] {}{}", s.id, s.name, marker);
    }
}

fn cmd_select_server(ctx: &DbConnection, arg: &str) {
    if arg.is_empty() {
        eprintln!("Usage: /server <id>");
        return;
    }

    // Try by ID first, then by name
    let server = if let Ok(id) = arg.parse::<u64>() {
        ctx.db.server().id().find(&id)
    } else {
        ctx.db
            .server()
            .iter()
            .find(|s| s.name.to_lowercase() == arg.to_lowercase())
    };

    if let Some(s) = server {
        CURRENT_SERVER_ID.store(s.id, Ordering::Relaxed);
        CURRENT_CHANNEL_ID.store(0, Ordering::Relaxed);
        println!("Active server: {} (id={})", s.name, s.id);

        // Show channels
        cmd_list_channels(ctx);

        // Auto-select first text channel
        if let Some(ch) = ctx
            .db
            .channel()
            .iter()
            .find(|c| c.server_id == s.id && c.channel_type == ChannelType::Text)
        {
            CURRENT_CHANNEL_ID.store(ch.id, Ordering::Relaxed);
            println!("Active channel: #{} (id={})", ch.name, ch.id);
        }
    } else {
        eprintln!("Server not found: {arg}");
    }
}

fn cmd_join_server(ctx: &DbConnection, arg: &str) {
    if arg.is_empty() {
        eprintln!("Usage: /join <server_id>");
        return;
    }
    let Ok(id) = arg.parse::<u64>() else {
        eprintln!("Invalid server ID: {arg}");
        return;
    };
    if let Err(e) = ctx.reducers.join_server(id) {
        eprintln!("Failed to join server: {e}");
    } else {
        println!("Joined server {id}");
    }
}

fn cmd_leave_server(ctx: &DbConnection, arg: &str) {
    if arg.is_empty() {
        eprintln!("Usage: /leave <server_id>");
        return;
    }
    let Ok(id) = arg.parse::<u64>() else {
        eprintln!("Invalid server ID: {arg}");
        return;
    };
    if let Err(e) = ctx.reducers.leave_server(id) {
        eprintln!("Failed to leave server: {e}");
    } else {
        println!("Left server {id}");
    }
}

fn cmd_create_server(ctx: &DbConnection, arg: &str) {
    if arg.is_empty() {
        eprintln!("Usage: /create-server <name>");
        return;
    }
    if let Err(e) = ctx.reducers.create_server(arg.to_string(), None) {
        eprintln!("Failed to create server: {e}");
    }
}

fn cmd_list_channels(ctx: &DbConnection) {
    let Some(server_id) = current_server() else {
        eprintln!("No server selected. Use /server <id> first.");
        return;
    };
    let mut channels: Vec<_> = ctx
        .db
        .channel()
        .iter()
        .filter(|c| c.server_id == server_id)
        .collect();
    channels.sort_by_key(|c| c.position);
    if channels.is_empty() {
        println!("No channels in this server.");
        return;
    }
    println!("Channels:");
    for ch in &channels {
        let marker = if current_channel() == Some(ch.id) {
            " *"
        } else {
            ""
        };
        let type_tag = match ch.channel_type {
            ChannelType::Text => "",
            ChannelType::Voice => " [voice]",
            ChannelType::Announcement => " [ann]",
            ChannelType::AgentLog => " [agent]",
        };
        println!("  [{}] #{}{}{}", ch.id, ch.name, type_tag, marker);
    }
}

fn cmd_select_channel(ctx: &DbConnection, arg: &str) {
    if arg.is_empty() {
        eprintln!("Usage: /channel <id>");
        return;
    }
    let Some(server_id) = current_server() else {
        eprintln!("No server selected.");
        return;
    };

    let channel = if let Ok(id) = arg.parse::<u64>() {
        ctx.db.channel().id().find(&id)
    } else {
        ctx.db
            .channel()
            .iter()
            .find(|c| c.server_id == server_id && c.name.to_lowercase() == arg.to_lowercase())
    };

    if let Some(ch) = channel {
        CURRENT_CHANNEL_ID.store(ch.id, Ordering::Relaxed);
        println!("Active channel: #{} (id={})", ch.name, ch.id);

        // Show recent messages
        let mut msgs: Vec<_> = ctx
            .db
            .message()
            .iter()
            .filter(|m| m.channel_id == ch.id && m.thread_id.is_none())
            .collect();
        msgs.sort_by_key(|m| m.created_at);
        let recent: Vec<_> = msgs
            .iter()
            .rev()
            .take(25)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if !recent.is_empty() {
            println!("--- Recent messages ---");
            for msg in recent {
                print_message(ctx, msg);
            }
            println!("--- End ---");
        }
    } else {
        eprintln!("Channel not found: {arg}");
    }
}

fn cmd_create_channel(ctx: &DbConnection, arg: &str) {
    if arg.is_empty() {
        eprintln!("Usage: /create-channel <name>");
        return;
    }
    let Some(server_id) = current_server() else {
        eprintln!("No server selected.");
        return;
    };
    if let Err(e) = ctx
        .reducers
        .create_channel(server_id, arg.to_string(), ChannelType::Text)
    {
        eprintln!("Failed to create channel: {e}");
    }
}

fn cmd_list_threads(ctx: &DbConnection) {
    let Some(ch_id) = current_channel() else {
        eprintln!("No channel selected.");
        return;
    };
    let threads: Vec<_> = ctx
        .db
        .thread()
        .iter()
        .filter(|t| t.channel_id == ch_id && !t.archived)
        .collect();
    if threads.is_empty() {
        println!("No active threads in this channel.");
        return;
    }
    println!("Threads:");
    for t in &threads {
        let creator = user_display_name(ctx, &t.creator);
        println!("  [{}] {} (by {})", t.id, t.name, creator);
    }
}

fn cmd_create_thread(ctx: &DbConnection, arg: &str) {
    if arg.is_empty() {
        eprintln!("Usage: /thread <name> | <initial message>");
        return;
    }
    let Some(ch_id) = current_channel() else {
        eprintln!("No channel selected.");
        return;
    };
    let parts: Vec<&str> = arg.splitn(2, '|').collect();
    let name = parts[0].trim().to_string();
    let msg = parts
        .get(1)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| name.clone());
    if let Err(e) = ctx.reducers.create_thread(ch_id, name, msg) {
        eprintln!("Failed to create thread: {e}");
    }
}

fn cmd_list_members(ctx: &DbConnection) {
    let Some(server_id) = current_server() else {
        eprintln!("No server selected.");
        return;
    };
    let members: Vec<_> = ctx
        .db
        .server_member()
        .iter()
        .filter(|m| m.server_id == server_id)
        .collect();
    if members.is_empty() {
        println!("No members.");
        return;
    }
    println!("Members ({}):", members.len());
    for m in &members {
        let name = user_display_name(ctx, &m.identity);
        let online = ctx
            .db
            .user_presence()
            .identity()
            .find(&m.identity)
            .map(|p| if p.online { " (online)" } else { "" })
            .unwrap_or("");
        println!("  {} [{:?}]{}", name, m.role, online);
    }
}

fn cmd_react(ctx: &DbConnection, arg: &str) {
    let parts: Vec<&str> = arg.splitn(2, ' ').collect();
    if parts.len() < 2 {
        eprintln!("Usage: /react <message_id> <emoji>");
        return;
    }
    let Ok(msg_id) = parts[0].parse::<u64>() else {
        eprintln!("Invalid message ID: {}", parts[0]);
        return;
    };
    if let Err(e) = ctx.reducers.add_reaction(msg_id, parts[1].to_string()) {
        eprintln!("Failed to react: {e}");
    }
}

fn cmd_register_agent(ctx: &DbConnection, arg: &str) {
    if arg.is_empty() {
        eprintln!("Usage: /agent <name> | <description>");
        return;
    }
    let parts: Vec<&str> = arg.splitn(2, '|').collect();
    let name = parts[0].trim().to_string();
    let desc = parts
        .get(1)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "Agent".to_string());
    if let Err(e) = ctx.reducers.register_agent(name, desc) {
        eprintln!("Failed to register agent: {e}");
    } else {
        println!("Registered as agent.");
    }
}

fn cmd_who(ctx: &DbConnection) {
    let online: Vec<_> = ctx.db.user_presence().iter().filter(|p| p.online).collect();
    if online.is_empty() {
        println!("No users online.");
        return;
    }
    println!("Online users ({}):", online.len());
    for p in &online {
        let name = user_display_name(ctx, &p.identity);
        let status = p.status_text.as_deref().unwrap_or("");
        if status.is_empty() {
            println!("  {}", name);
        } else {
            println!("  {} - {}", name, status);
        }
    }
}

fn cmd_history(ctx: &DbConnection, arg: &str) {
    let Some(ch_id) = current_channel() else {
        eprintln!("No channel selected.");
        return;
    };
    let count: usize = arg.parse().unwrap_or(25);
    let mut msgs: Vec<_> = ctx
        .db
        .message()
        .iter()
        .filter(|m| m.channel_id == ch_id && m.thread_id.is_none())
        .collect();
    msgs.sort_by_key(|m| m.created_at);
    let recent: Vec<_> = msgs
        .iter()
        .rev()
        .take(count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if recent.is_empty() {
        println!("No messages.");
        return;
    }
    println!("--- Last {} messages ---", recent.len());
    for msg in recent {
        print_message(ctx, msg);
    }
    println!("--- End ---");
}
