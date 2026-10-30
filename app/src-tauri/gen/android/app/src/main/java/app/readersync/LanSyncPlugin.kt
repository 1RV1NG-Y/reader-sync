package app.readersync

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import androidx.appcompat.app.AlertDialog
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import android.graphics.Color
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class ReadingBarsArgs {
  var reading: Boolean = false
  var color: String = "#ffffff"
  var dark: Boolean = false
  var hideStatusBar: Boolean = true
}

@TauriPlugin
class LanSyncPlugin(private val activity: Activity) : Plugin(activity) {
  /** The reader's page colour behind the system bars, and the status bar hidden while reading. */
  @Command
  fun setReadingBars(invoke: Invoke) {
    val args = invoke.parseArgs(ReadingBarsArgs::class.java)
    android.util.Log.i("ReadingBars", "reading=${args.reading} hideStatusBar=${args.hideStatusBar} color=${args.color} dark=${args.dark}")
    activity.runOnUiThread {
      try {
        val top = (activity as? MainActivity)?.setReadingBars(
          if (args.reading) MainActivity.ReadingBars(Color.parseColor(args.color), args.dark, args.hideStatusBar) else null) ?: 0f
        invoke.resolve(JSObject().put("top", top.toDouble()))
      } catch (e: IllegalArgumentException) {
        invoke.reject(e.message)
      }
    }
  }

  @Command
  fun showControls(invoke: Invoke) {
    activity.runOnUiThread {
      val enabled = LanSyncService.enabled(activity)
      val exempt = (activity.getSystemService(Activity.POWER_SERVICE) as PowerManager).isIgnoringBatteryOptimizations(activity.packageName)
      val notifications = Build.VERSION.SDK_INT < 33 || ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
      val failure = LanSyncService.preferences(activity).getString("last-error", null)
      val choices = mutableListOf(if (enabled) "Pause background sync" else "Start / resume background sync")
      if (!exempt) choices.add("Allow listening during device idle…")
      if (!notifications) choices.add("Allow sync notifications…")
      AlertDialog.Builder(activity).setTitle("Background LAN sync")
        .setMessage("${if (enabled) "Enabled" else "Paused"}. While enabled, reader-sync keeps a local-network listener and CPU/Wi-Fi locks active so paired computers can send books and positions with the screen off. This uses extra battery.\n\n${if (exempt) "Battery exemption is enabled." else "Android Doze can still block the network. For reliable receipt during idle, you can opt into a battery exemption."}\n\n${if (notifications) "Sync notifications are allowed." else "Notifications are blocked. Android can run the listener, but hides its notification; allow notifications below to see its status and Pause action."}\n\nPausing stops listening after the reader leaves the screen. No cloud service is used.${if (failure == null) "" else "\n\n$failure"}")
        .setPositiveButton("Controls") { _, _ ->
          AlertDialog.Builder(activity).setTitle("Background LAN sync")
            .setItems(choices.toTypedArray()) { _, index ->
              try {
                when (choices[index]) {
                  choices[0] -> if (enabled) LanSyncService.pause(activity) else {
                    LanSyncService.resume(activity)
                    if (!notifications && Build.VERSION.SDK_INT >= 33) ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 47110)
                  }
                  "Allow listening during device idle…" -> AlertDialog.Builder(activity)
                    .setTitle("Allow LAN sync while idle?")
                    .setMessage("Android normally suspends network access during Doze, even with a foreground service. Exempting reader-sync lets your paired devices send books while this phone is idle and can increase battery use. You can undo this in Android battery settings.")
                    .setPositiveButton("Open Android approval") { _, _ ->
                      activity.startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:${activity.packageName}")))
                    }.setNegativeButton("Cancel", null).show()
                  else -> if (Build.VERSION.SDK_INT >= 33) ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 47110)
                }
              } catch (e: Exception) {
                LanSyncService.preferences(activity).edit().putBoolean("enabled", false).putString("last-error", e.message).apply()
                AlertDialog.Builder(activity).setTitle("Could not change LAN sync").setMessage(e.message).setPositiveButton("OK", null).show()
              }
            }.setNegativeButton("Cancel", null).show()
        }.setNegativeButton("Done", null).show()
      invoke.resolve()
    }
  }
}
