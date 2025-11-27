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

/// Checks if the media player is currently playing.
async fn is_playing() -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
   let connection = Connection::session().await?;
   let dbus_proxy = zbus::fdo::DBusProxy::new(&connection).await?;
   let names = dbus_proxy.list_names().await?;

   let mpris_service = names
      .iter()
      .find(|name| name.starts_with("org.mpris.MediaPlayer2."));

   let Some(service_name) = mpris_service else {
      return Ok(false);
   };

   // Get the PlaybackStatus property
   let path = zbus::zvariant::ObjectPath::from_str_unchecked("/org/mpris/MediaPlayer2");
   let interface = "org.mpris.MediaPlayer2.Player";
   let property = "PlaybackStatus";
   
   let reply = connection
      .call_method(
         Some(service_name.as_str()),
         &path,
         Some("org.freedesktop.DBus.Properties"),
         "Get",
         &(interface, property),
      )
      .await?;

   let body = reply.body();
   // Properties.Get returns a Variant containing the actual value
   // Try to deserialize directly - zbus should handle the Variant unwrapping
   let status: String = match body.deserialize() {
      Ok(s) => s,
      Err(_) => {
         // If direct deserialization fails, try extracting from Value
         let value: zbus::zvariant::Value = body.deserialize()?;
         match value {
            zbus::zvariant::Value::Str(s) => s.to_string(),
            _ => return Ok(false), // Can't determine status
         }
      }
   };
   
   Ok(status == "Playing")
}

/// Sends a play/pause toggle command to the active media player via MPRIS.
pub async fn send_play_pause() {
   if let Err(e) = send_mpris_command("PlayPause").await {
      warn!("Failed to send play/pause command: {e}");
   } else {
      debug!("Sent play/pause command to media player");
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

async fn send_mpris_command(method: &str) -> Result<Option<(String, bool)>, Box<dyn std::error::Error + Send + Sync>> {
   // Connect to the session bus
   let connection = Connection::session().await?;

   // List all MPRIS services
   let dbus_proxy = zbus::fdo::DBusProxy::new(&connection).await?;
   let names = dbus_proxy.list_names().await?;

   // Find all MPRIS media players
   let mpris_services: Vec<_> = names
      .iter()
      .filter(|name| name.starts_with("org.mpris.MediaPlayer2."))
      .collect();

   if mpris_services.is_empty() {
      debug!("No MPRIS media player found");
      return Ok(None); // Not an error if no player is active
   }

   debug!("Found {} MPRIS player(s): {:?}", mpris_services.len(), mpris_services);

   // Try to find the active player (one that's currently playing)
   let mut active_player = None;
   let path = zbus::zvariant::ObjectPath::from_str_unchecked("/org/mpris/MediaPlayer2");
   let interface = "org.mpris.MediaPlayer2.Player";

   for service_name in &mpris_services {
      // Check if this player is playing BEFORE we pause it
      let was_playing = is_player_playing(service_name.as_str()).await.unwrap_or(false);
      debug!("Player {} has status: {}", service_name, if was_playing { "Playing" } else { "Not Playing" });
      
      if was_playing {
         active_player = Some((service_name.as_str(), true));
         break;
      }
   }

   // If no playing player found, use the first one (fallback)
   let (preferred_service, was_playing) = active_player
      .unwrap_or_else(|| {
         let first = mpris_services.first().map(|s| (s.as_str(), false)).unwrap();
         (first.0, false)
      });
   
   let preferred_service_name = preferred_service.to_string();

   if was_playing {
      debug!("Using active player: {}", preferred_service);
   } else {
      debug!("No active player found, using first available: {}", preferred_service);
   }

   // Call the method using zbus's call API
   debug!("Sending {} command to {} at path {}", method, preferred_service, path.as_str());
   
   // Try the preferred player first
   let result = connection
      .call_method(
         Some(preferred_service),
         &path,
         Some(interface),
         method,
         &(),
      )
      .await;

   match result {
      Ok(reply) => {
         debug!("Successfully sent {} command to {} (reply: {:?})", method, preferred_service, reply);
         return Ok(Some((preferred_service_name, was_playing)));
      },
      Err(e) => {
         debug!("Failed to send {} command to preferred player {}: {}", method, preferred_service, e);
         // If preferred player fails, try others as fallback
         warn!("Preferred player failed, trying other players as fallback");
      },
   }

   // Fallback: try other players if preferred one failed
   let mut last_error = None;
   for service_name_to_try in mpris_services.iter() {
      // Skip the one we already tried
      if service_name_to_try.as_str() == preferred_service {
         continue;
      }

      // Check if this fallback player is playing
      let was_playing_fallback = is_player_playing(service_name_to_try.as_str()).await.unwrap_or(false);
      
      debug!("Trying fallback player: {} (playing: {})", service_name_to_try, was_playing_fallback);
      let result = connection
         .call_method(
            Some(service_name_to_try.as_str()),
            &path,
            Some(interface),
            method,
            &(),
         )
         .await;

      match result {
         Ok(reply) => {
            debug!("Successfully sent {} command to fallback player {} (reply: {:?})", method, service_name_to_try, reply);
            return Ok(Some((service_name_to_try.as_str().to_string(), was_playing_fallback)));
         },
         Err(e) => {
            debug!("Failed to send {} command to {}: {}", method, service_name_to_try, e);
            last_error = Some(e);
         },
      }
   }

   // If we get here, all players failed
   if let Some(e) = last_error {
      warn!("Failed to send {} command to all MPRIS players. Last error: {}", method, e);
      Err(Box::new(e))
   } else {
      warn!("No MPRIS players available to send {} command", method);
      Err("No MPRIS players available".into())
   }
}

