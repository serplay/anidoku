package com.anidoku.app

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat

/**
 * Foreground service that keeps the process (and the Rust download engine
 * running inside it) alive while episodes download. Started/stopped from Rust
 * via [setActive] whenever the count of queued/downloading rows crosses zero.
 */
class DownloadForegroundService : Service() {
  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    createChannel()
    val tap = PendingIntent.getActivity(
      this, 0,
      Intent(this, MainActivity::class.java),
      PendingIntent.FLAG_IMMUTABLE
    )
    val notification = NotificationCompat.Builder(this, CHANNEL_ID)
      .setSmallIcon(android.R.drawable.stat_sys_download)
      .setContentTitle("AniDoku")
      .setContentText("Downloading episodes…")
      .setOngoing(true)
      .setContentIntent(tap)
      .build()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
    } else {
      startForeground(NOTIFICATION_ID, notification)
    }
    // The Rust engine owns the work; if the system kills us there is nothing
    // to restart here (downloads recover from checkpoints on next app start).
    return START_NOT_STICKY
  }

  private fun createChannel() {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      val channel = NotificationChannel(
        CHANNEL_ID, "Downloads", NotificationManager.IMPORTANCE_LOW
      )
      channel.description = "Shown while episodes are downloading"
      (getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager)
        .createNotificationChannel(channel)
    }
  }

  companion object {
    private const val CHANNEL_ID = "anidoku.downloads"
    private const val NOTIFICATION_ID = 4210

    /** Called from Rust (JNI) — see src-tauri/src/android.rs. */
    @JvmStatic
    fun setActive(context: Context, active: Boolean) {
      val intent = Intent(context, DownloadForegroundService::class.java)
      if (active) {
        // Only legal while the app is foregrounded (Android 12+), which holds:
        // downloads are enqueued from the UI.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
          context.startForegroundService(intent)
        } else {
          context.startService(intent)
        }
      } else {
        context.stopService(intent)
      }
    }
  }
}
