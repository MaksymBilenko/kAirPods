//! Media control module for sending play/pause commands via MPRIS.
//!
//! This module provides functionality to control media playback using the
//! MPRIS (Media Player Remote Interfacing Specification) D-Bus interface.

use log::{debug, warn};
use parking_lot::Mutex;
use zbus::Connection;

/// Tracks which players we paused (so we can resume all of them)
static PAUSED_PLAYERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Sends a play command to all players we previously paused.
/// Only plays if we previously paused the media.
pub async fn send_play() {
   // Get all players we paused
   let paused_players = PAUSED_PLAYERS.lock().clone();
   
   if paused_players.is_empty() {
      debug!("No media was paused by us, skipping play command");
      return;
   }

   debug!("Resuming {} previously paused player(s): {:?}", paused_players.len(), paused_players);
   
   // Resume all paused players
   let mut successful = 0;
   
   for player_name in &paused_players {
      match send_mpris_command_to_player("Play", player_name).await {
         Ok(_) => {
            debug!("Successfully resumed player: {}", player_name);
            successful += 1;
         },
         Err(e) => {
            warn!("Failed to resume player {}: {}", player_name, e);
         },
      }
   }
   
   debug!("Resumed {}/{} players successfully", successful, paused_players.len());
   
   // Clear the stored players since we've resumed them all
   PAUSED_PLAYERS.lock().clear();
}

/// Sends a pause command to all playing media players via MPRIS.
/// Stores all players that were paused (only if they were playing).
pub async fn send_pause() {
   // Find all playing players and pause them all
   let connection = Connection::session().await;
   let Ok(connection) = connection else {
      warn!("Failed to connect to D-Bus session");
      return;
   };

   let dbus_proxy = match zbus::fdo::DBusProxy::new(&connection).await {
      Ok(proxy) => proxy,
      Err(e) => {
         warn!("Failed to create D-Bus proxy: {}", e);
         return;
      }
   };

   let names = match dbus_proxy.list_names().await {
      Ok(names) => names,
      Err(e) => {
         warn!("Failed to list D-Bus names: {}", e);
         return;
      }
   };

   // Find all MPRIS media players
   let mpris_services: Vec<_> = names
      .iter()
      .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
      .collect();

   if mpris_services.is_empty() {
      debug!("No MPRIS media players found");
      return;
   }

   debug!("Found {} MPRIS player(s), checking which are playing", mpris_services.len());

   let mut paused_players = Vec::new();

   // Check each player and pause all that are playing
   for service_name in &mpris_services {
      // Check if this player is playing
      if let Ok(was_playing) = is_player_playing(service_name.as_str()).await {
         if was_playing {
            debug!("Player {} is playing, pausing it", service_name);
            // Pause this player
            match send_mpris_command_to_player("Pause", service_name.as_str()).await {
               Ok(_) => {
                  debug!("Successfully paused player: {}", service_name);
                  paused_players.push(service_name.as_str().to_string());
               },
               Err(e) => {
                  warn!("Failed to pause player {}: {}", service_name, e);
               },
            }
         } else {
            debug!("Player {} is not playing, skipping", service_name);
         }
      } else {
         debug!("Could not check playback status for player {}, skipping", service_name);
      }
   }

   if paused_players.is_empty() {
      debug!("No playing players found to pause");
   } else {
      debug!("Paused {} player(s), storing for resume: {:?}", paused_players.len(), paused_players);
      // Store all paused players
      *PAUSED_PLAYERS.lock() = paused_players;
   }
}

/// Checks if a specific player is currently playing.
async fn is_player_playing(service_name: &str) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
   let connection = Connection::session().await?;
   let path = zbus::zvariant::ObjectPath::from_str_unchecked("/org/mpris/MediaPlayer2");
   let interface = "org.mpris.MediaPlayer2.Player";
   let property = "PlaybackStatus";
   
   let reply = connection
      .call_method(
         Some(service_name),
         &path,
         Some("org.freedesktop.DBus.Properties"),
         "Get",
         &(interface, property),
      )
      .await?;

   let body = reply.body();
   let variant: zbus::zvariant::Value = body.deserialize()?;
   let status = match variant {
      zbus::zvariant::Value::Str(s) => s.to_string(),
      _ => {
         if let Ok(s) = String::try_from(variant) {
            s
         } else {
            return Ok(false);
         }
      }
   };
   
   Ok(status == "Playing")
}

/// Sends a command to a specific player by service name.
async fn send_mpris_command_to_player(
   method: &str,
   service_name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
   let connection = Connection::session().await?;
   let path = zbus::zvariant::ObjectPath::from_str_unchecked("/org/mpris/MediaPlayer2");
   let interface = "org.mpris.MediaPlayer2.Player";

   debug!("Sending {} command to specific player: {}", method, service_name);

   connection
      .call_method(
         Some(service_name),
         &path,
         Some(interface),
         method,
         &(),
      )
      .await?;

   Ok(())
}

