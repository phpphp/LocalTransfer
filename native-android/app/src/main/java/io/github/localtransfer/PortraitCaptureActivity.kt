package io.github.localtransfer

import android.app.Activity
import android.os.Bundle
import android.view.KeyEvent
import com.journeyapps.barcodescanner.CaptureManager
import com.journeyapps.barcodescanner.DecoratedBarcodeView

/** 竖屏卡片式扫码页。
 *  库自带的 CaptureActivity 在其 Manifest 里声明 sensorLandscape（横屏满屏
 *  布局，提示文字也是为横屏排的）——不符合"竖着拿手机扫码"的使用方式，
 *  所以自己写 Activity：DecoratedBarcodeView 只占屏幕中间的卡片区域，
 *  上下是正常竖屏排版（提示文字横向居中）。 */
class PortraitCaptureActivity : Activity() {
    private lateinit var capture: CaptureManager
    private lateinit var barcode: DecoratedBarcodeView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_scan)
        // id 必须叫 zxing_barcode_scanner / zxing_status_view（CaptureManager 按名查找）
        barcode = findViewById(R.id.zxing_barcode_scanner)
        capture = CaptureManager(this, barcode)
        capture.initializeFromIntent(intent, savedInstanceState)
        capture.decode()
    }

    override fun onResume() { super.onResume(); capture.onResume() }
    override fun onPause() { super.onPause(); capture.onPause() }
    override fun onDestroy() { super.onDestroy(); capture.onDestroy() }
    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        capture.onSaveInstanceState(outState)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int, permissions: Array<String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        capture.onRequestPermissionsResult(requestCode, permissions, grantResults)
    }

    /** 音量键缩放等（DecoratedBarcodeView 内置行为） */
    override fun onKeyDown(keyCode: Int, event: KeyEvent?): Boolean =
        barcode.onKeyDown(keyCode, event) || super.onKeyDown(keyCode, event)
}
