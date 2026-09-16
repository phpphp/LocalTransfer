package io.github.localtransfer

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder

/**
 * 前台服务：持住进程，应用退到后台/息屏时 HTTP 服务与发现循环继续工作。
 * API 33+ 需 POST_NOTIFICATIONS 运行时权限（MainActivity 启动时请求）。
 */
class TransferService : Service() {

    override fun onCreate() {
        super.onCreate()
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        val channelId = "localtransfer_bg"
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(
                NotificationChannel(channelId, "后台接收",
                    NotificationManager.IMPORTANCE_LOW).apply {
                    description = "保持 LocalTransfer 可接收文件"
                    setShowBadge(false)
                })
        }
        val pi = PendingIntent.getActivity(
            this, 0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val note: Notification = if (Build.VERSION.SDK_INT >= 26) {
            Notification.Builder(this, channelId)
                .setContentTitle("LocalTransfer 正在运行")
                .setContentText("局域网设备可向你发送文件")
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentIntent(pi)
                .setOngoing(true)
                .build()
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this)
                .setContentTitle("LocalTransfer 正在运行")
                .setContentText("局域网设备可向你发送文件")
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentIntent(pi)
                .setOngoing(true)
                .build()
        }
        startForeground(1, note)
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
