package io.github.localtransfer

import android.content.pm.ActivityInfo
import android.os.Bundle
import com.journeyapps.barcodescanner.CaptureActivity

/** 横屏扫码（zxing 默认竖屏）。
 *  根因：CaptureManager.onCreate 读 SCAN_ORIENTATION_LOCKED（默认 true），
 *  把"启动瞬间的屏幕方向"锁死——request 横屏是异步的，它读到的还是竖屏。
 *  startScan 已 setOrientationLocked(false) 关掉库锁；这里 super 前后各
 *  request 一次兜底（Manifest sensorLandscape + 运行时双保险）。 */
class LandscapeCaptureActivity : CaptureActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        super.onCreate(savedInstanceState)
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
    }
}
