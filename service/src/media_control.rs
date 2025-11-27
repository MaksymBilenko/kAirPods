//! Media control module for sending play/pause commands via MPRIS.
//!
//! This module provides functionality to control media playback using the
//! MPRIS (Media Player Remote Interfacing Specification) D-Bus interface.

use std::sync::atomic::{AtomicBool, Ordering};

use log::{debug, warn};
use zbus::Connection;

/// Tracks whether we paused the media (so we only resume if we paused it)
static WE_PAUSED: AtomicBool = AtomicBool::new(false);

/// Sends a play command to the active media player via MPRIS.
/// Only plays if we previously paused the media.
pub async fn send_play() {
   // Only play if we were the ones who paused it
   if !WE_PAUSED.load(Ordering::Relaxed) {
      debug!("Media was not paused by us, skipping play command");
      return;
   }

   if let Err(e) = send_mpris_command("Play").await {
      warn!("Failed to send play command: {e}");
   } else {
      debug!("Sent play command to media player");
      // Clear the flag since we've resumed
      WE_PAUSED.store(false, Ordering::Relaxed);
   }
}

/// Sends a pause command to the active media player via MPRIS.
/// Marks that we paused the media (only if it was playing).
pub async fn send_pause() {
   // Check if media is currently playing before pausing
   let was_playing = is_playing().await.unwrap_or(false);
   
   if let Err(e) = send_mpris_command("Pause").await {
      warn!("Failed to send pause command: {e}");
   } else {
      debug!("Sent pause command to media player");
      // Only mark that we paused it if it was actually playing
      if was_playing {
         WE_PAUSED.store(true, Ordering::Relaxed);
      }
   }
}

/// Checks if the media player is currently playing.
async fn is_playing() -> Result<bool, Box<dyn std::error::Error>> {
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

async fn send_mpris_command(method: &str) -> Result<(), Box<dyn std::error::Error>> {
   // Connect to the session bus
   let connection = Connection::session().await?;

   // List all MPRIS services
   let dbus_proxy = zbus::fdo::DBusProxy::new(&connection).await?;
   let names = dbus_proxy.list_names().await?;

   // Find the first active MPRIS media player
   let mpris_service = names
      .iter()
      .find(|name| name.starts_with("org.mpris.MediaPlayer2."));

   let Some(service_name) = mpris_service else {
      debug!("No active MPRIS media player found");
      return Ok(()); // Not an error if no player is active
   };

   // Call the method using zbus's call API
   let path = zbus::zvariant::ObjectPath::from_str_unchecked("/org/mpris/MediaPlayer2");
   let interface = "org.mpris.MediaPlayer2.Player";
   
   connection
      .call_method(
         Some(service_name.as_str()),
         &path,
         Some(interface),
         method,
         &(),
      )
      .await?;

   Ok(())
}

