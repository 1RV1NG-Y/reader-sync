package app.readersync

import android.content.Context
import android.content.res.Configuration
import android.graphics.Color
import android.net.wifi.WifiManager
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  // Wi-Fi drivers drop multicast packets unless an app holds this lock; mDNS discovery of
  // paired devices depends on it (as in Bits).
  private var multicastLock: WifiManager.MulticastLock? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
    multicastLock = wifi.createMulticastLock("reader-sync-mdns").apply {
      setReferenceCounted(false)
      acquire()
    }
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

  override fun onDestroy() {
    multicastLock?.release()
    super.onDestroy()
  }

  // The manifest handles uiMode changes itself (no activity restart), so follow them here.
  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    applyBarColors(newConfig)
  }

  /** The strips behind the system bars follow the system light/dark mode. */
  private fun applyBarColors(config: Configuration) {
    val night = (config.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
    val content = findViewById<android.view.View>(android.R.id.content)
    content.setBackgroundColor(Color.parseColor(if (night) "#222326" else "#ffffff"))
    WindowCompat.getInsetsController(window, content).apply {
      isAppearanceLightStatusBars = !night
      isAppearanceLightNavigationBars = !night
    }
  }
}
