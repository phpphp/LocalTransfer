package io.github.localtransfer

import android.content.ContentUris
import android.net.Uri
import android.provider.MediaStore
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil.compose.AsyncImage
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.GlobalScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

private val SelBlue = Color(0xFF4F46E5)

/** 一条媒体（相册网格单元） */
data class MediaItem(
    val uri: Uri,
    val name: String,
    val album: String,       // 相册名（bucket）
    val isVideo: Boolean,
    val durationMs: Long,    // 视频时长（照片 0）
    val dateTaken: Long,
)

/** 应用内相册选择器（微信式）：网格缩略图 + 相册分类 + 多选编号 + 确定发送。
 *  系统的 PickVisualMedia 在部分机型会退化成文件选择器（无 Photo Picker 模块），
 *  自建读 MediaStore 的体验一致且可控。
 */
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
fun GalleryPicker(onSend: (List<Uri>) -> Unit, onClose: () -> Unit) {
    val ctx = LocalContext.current
    var all by remember { mutableStateOf<List<MediaItem>>(emptyList()) }
    var album by remember { mutableStateOf("全部") }
    var selection by remember { mutableStateOf<List<MediaItem>>(emptyList()) }
    var albumMenu by remember { mutableStateOf(false) }

    // 读媒体库（图片+视频，按时间倒序）
    LaunchedEffect(Unit) {
        withContext(Dispatchers.IO) {
            val list = mutableListOf<MediaItem>()
            // 图片（注意：images 表没有 duration 列——部分系统查了直接崩，
            // 模拟器实测 SQLiteException no such column: duration）
            runCatching {
                ctx.contentResolver.query(
                    MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
                    arrayOf(
                        MediaStore.MediaColumns._ID,
                        MediaStore.MediaColumns.DISPLAY_NAME,
                        MediaStore.MediaColumns.BUCKET_DISPLAY_NAME,
                        MediaStore.MediaColumns.DATE_MODIFIED,
                    ),
                    null, null,
                    "${MediaStore.MediaColumns.DATE_MODIFIED} DESC")?.use { c ->
                    val idCol = c.getColumnIndexOrThrow(MediaStore.MediaColumns._ID)
                    val nameCol = c.getColumnIndexOrThrow(MediaStore.MediaColumns.DISPLAY_NAME)
                    val albumCol = c.getColumnIndexOrThrow(MediaStore.MediaColumns.BUCKET_DISPLAY_NAME)
                    val dateCol = c.getColumnIndexOrThrow(MediaStore.MediaColumns.DATE_MODIFIED)
                    while (c.moveToNext()) {
                        list.add(MediaItem(
                            ContentUris.withAppendedId(
                                MediaStore.Images.Media.EXTERNAL_CONTENT_URI, c.getLong(idCol)),
                            c.getString(nameCol) ?: "",
                            c.getString(albumCol) ?: "图片",
                            false, 0, c.getLong(dateCol)))
                    }
                }
            }.onFailure { android.util.Log.w("LT", "图片库读取失败", it) }
            // 视频（duration 只有视频表有）
            runCatching {
                ctx.contentResolver.query(
                    MediaStore.Video.Media.EXTERNAL_CONTENT_URI,
                    arrayOf(
                        MediaStore.MediaColumns._ID,
                        MediaStore.MediaColumns.DISPLAY_NAME,
                        MediaStore.MediaColumns.BUCKET_DISPLAY_NAME,
                        MediaStore.MediaColumns.DATE_MODIFIED,
                        MediaStore.MediaColumns.DURATION,
                    ),
                    null, null,
                    "${MediaStore.MediaColumns.DATE_MODIFIED} DESC")?.use { c ->
                    val idCol = c.getColumnIndexOrThrow(MediaStore.MediaColumns._ID)
                    val nameCol = c.getColumnIndexOrThrow(MediaStore.MediaColumns.DISPLAY_NAME)
                    val albumCol = c.getColumnIndexOrThrow(MediaStore.MediaColumns.BUCKET_DISPLAY_NAME)
                    val dateCol = c.getColumnIndexOrThrow(MediaStore.MediaColumns.DATE_MODIFIED)
                    val durCol = c.getColumnIndexOrThrow(MediaStore.MediaColumns.DURATION)
                    while (c.moveToNext()) {
                        list.add(MediaItem(
                            ContentUris.withAppendedId(
                                MediaStore.Video.Media.EXTERNAL_CONTENT_URI, c.getLong(idCol)),
                            c.getString(nameCol) ?: "",
                            c.getString(albumCol) ?: "视频",
                            true, c.getLong(durCol), c.getLong(dateCol)))
                    }
                }
            }.onFailure { android.util.Log.w("LT", "视频库读取失败", it) }
            list.sortByDescending { it.dateTaken }
            withContext(kotlinx.coroutines.Dispatchers.Main) { all = list }
        }
    }

    val albums = remember(all) {
        listOf("全部") + all.map { it.album }.distinct().sorted()
    }
    val shown = if (album == "全部") all else all.filter { it.album == album }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        // 顶栏：关闭 + 相册下拉 + 已选数 + 发送
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onClose) {
                Icon(Icons.AutoMirrored.Rounded.ArrowBack, "关闭")
            }
            Box {
                Row(Modifier.clickable { albumMenu = true }.padding(6.dp),
                    verticalAlignment = Alignment.CenterVertically) {
                    Text(album, fontWeight = FontWeight.Medium, fontSize = 16.sp)
                    Icon(Icons.Rounded.KeyboardArrowDown, null,
                        modifier = Modifier.size(20.dp))
                }
                DropdownMenu(expanded = albumMenu,
                    onDismissRequest = { albumMenu = false }) {
                    albums.forEach { a ->
                        DropdownMenuItem(text = {
                            Text(if (a == "全部") "全部（${all.size}）"
                                 else "$a（${all.count { it.album == a }}）")
                        }, onClick = { album = a; albumMenu = false })
                    }
                }
            }
            Spacer(Modifier.weight(1f))
            Text("已选 ${selection.size}", fontSize = 13.sp,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(10.dp))
            Button(
                onClick = {
                    onSend(selection.map { it.uri })
                },
                enabled = selection.isNotEmpty(),
            ) { Text("发送") }
        }
        // 网格
        LazyVerticalGrid(
            columns = GridCells.Fixed(3),
            modifier = Modifier.fillMaxSize().padding(horizontal = 2.dp),
        ) {
            items(shown, key = { it.uri.toString() }) { m ->
                Box(Modifier.padding(2.dp).aspectRatio(1f)
                    .clip(RoundedCornerShape(4.dp))
                    .clickable {
                        selection = if (selection.contains(m))
                            selection - m else selection + m
                    }) {
                    AsyncImage(m.uri, m.name,
                        modifier = Modifier.fillMaxSize(),
                        contentScale = ContentScale.Crop)
                    if (m.isVideo) {
                        // 视频角标：时长 + 播放图标
                        Row(Modifier.align(Alignment.BottomStart)
                            .background(Color.Black.copy(alpha = 0.55f))
                            .padding(horizontal = 4.dp, vertical = 2.dp),
                            verticalAlignment = Alignment.CenterVertically) {
                            Icon(Icons.Rounded.PlayArrow, null,
                                modifier = Modifier.size(12.dp),
                                tint = Color.White)
                            Text(fmtDuration(m.durationMs), fontSize = 9.sp,
                                color = Color.White)
                        }
                    }
                    // 选择圈（带序号）
                    val idx = selection.indexOf(m)
                    Box(Modifier.align(Alignment.TopEnd).padding(5.dp)
                        .size(22.dp)
                        .clip(CircleShape)
                        .background(if (idx >= 0) SelBlue
                                    else Color.Black.copy(alpha = 0.3f)),
                        contentAlignment = Alignment.Center) {
                        if (idx >= 0) {
                            Text("${idx + 1}", fontSize = 11.sp, color = Color.White)
                        }
                    }
                }
            }
        }
    }
}

private fun fmtDuration(ms: Long): String {
    val s = ms / 1000
    return "%d:%02d".format(s / 60, s % 60)
}
