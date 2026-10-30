package app.readersync

import android.content.res.Configuration
import android.graphics.Color
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  /** While a book is open: the reader's page colour, and whether the status bar is hidden. */
  data class ReadingBars(val color: Int, val dark: Boolean, val hideStatusBar: Boolean)
  private var reading: ReadingBars? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Keep the web view clear of the status bar, navigation bar, cutout and keyboard (as in Bits).
    val content = findViewById<android.view.View>(android.R.id.content)
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime()
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
    applyBarColors(resources.configuration)
  }

  override fun onStart() {
    super.onStart()
    LanSyncRuntime.setVisible(this, true)
    try {
      LanSyncService.startIfEnabled(this)
    } catch (error: Exception) {
      // Background-start restrictions (or vendor policy) must never crash the reader.
      android.util.Log.e("LanSync", "Android rejected the foreground service start", error)
      LanSyncService.preferences(this).edit().putBoolean("enabled", false)
        .putString("last-error", "Android could not start background sync: ${error.message}").apply()
      android.widget.Toast.makeText(this, "Background LAN sync could not start. Open Devices → Background LAN sync to retry.", android.widget.Toast.LENGTH_LONG).show()
    }
  }

  // Android can bring the status bar back when another window (a dialog, the shade) takes focus.
  override fun onWindowFocusChanged(hasFocus: Boolean) {
    super.onWindowFocusChanged(hasFocus)
    if (hasFocus) applyBarColors(resources.configuration)
  }

  /** Called by the reader when a book opens or its theme changes (null when it closes). */
  fun setReadingBars(bars: ReadingBars?) {
    reading = bars
    applyBarColors(resources.configuration)
  }

  override fun onStop() {
    LanSyncRuntime.setVisible(this, false)
    super.onStop()
  }

  // The manifest handles uiMode changes itself (no activity restart), so follow them here.
  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    applyBarColors(newConfig)
  }

  /**
   * The strips behind the system bars follow the system light/dark mode, or the page while a
   * book is open. Reading can hide the status bar; a swipe from the top shows it for a moment
   * without moving the page.
   */
  private fun applyBarColors(config: Configuration) {
    val bars = reading
    val dark = bars?.dark ?: ((config.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES)
    val content = findViewById<android.view.View>(android.R.id.content)
    content.setBackgroundColor(bars?.color ?: Color.parseColor(if (dark) "#222326" else "#ffffff"))
    WindowCompat.getInsetsController(window, content).apply {
      isAppearanceLightStatusBars = !dark
      isAppearanceLightNavigationBars = !dark
      systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
      if (bars?.hideStatusBar == true) hide(WindowInsetsCompat.Type.statusBars()) else show(WindowInsetsCompat.Type.statusBars())
    }
  }
}
