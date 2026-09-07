package io.github.localtransfer

import android.content.pm.ActivityInfo
import android.os.Bundle
import com.journeyapps.barcodescanner.CaptureActivity

/** 横屏扫码（zxing 默认竖屏）。
 *  Manifest sensorLandscape 在部分 ROM 上被系统"锁定竖屏"覆盖，
 *  运行时再强制一次才可靠。 */
class LandscapeCaptureActivity : CaptureActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        super.onCreate(savedInstanceState)
    }
}
