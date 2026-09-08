package io.github.localtransfer

import android.content.pm.ActivityInfo
import android.os.Bundle
import com.journeyapps.barcodescanner.CaptureActivity

/** 横屏扫码（zxing 默认竖屏）。
 *  三层修复经验：
 *  ① CaptureManager 默认 SCAN_ORIENTATION_LOCKED=true 锁"启动瞬间方向"
 *    → startScan setOrientationLocked(false) 关库锁；
 *  ② sensorLandscape（=6）依赖系统"自动旋转"开关——用户关掉自动旋转时
 *    ROM 把它按竖屏处理 → 改用固定 LANDSCAPE（无视自动旋转设置）；
 *  ③ 运行时在 super 前后各 request 一次兜底。 */
class LandscapeCaptureActivity : CaptureActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE
        super.onCreate(savedInstanceState)
        requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE
    }
}
