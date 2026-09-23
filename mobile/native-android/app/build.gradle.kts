import java.util.Properties
import java.io.FileInputStream

plugins {
    id("com.android.application")
    // AGP 9 内置 Kotlin（org.jetbrains.kotlin.android 不再需要）
    id("org.jetbrains.kotlin.plugin.compose")
}

// 正式签名配置：keystore.properties + localtransfer.jks（均不入库）。
// 文件不存在时自动回落 debug 签名（开发机克隆后直接可构建）。
val keystoreProps = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.exists()) load(FileInputStream(f))
}

android {
    namespace = "io.github.localtransfer"
    compileSdk = 36

    defaultConfig {
        applicationId = "io.github.localtransfer"
        minSdk = 24
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }
    signingConfigs {
        if (keystoreProps.isNotEmpty()) {
            create("release") {
                storeFile = rootProject.file(keystoreProps.getProperty("storeFile"))
                storePassword = keystoreProps.getProperty("storePassword")
                keyAlias = keystoreProps.getProperty("keyAlias")
                keyPassword = keystoreProps.getProperty("keyPassword")
            }
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = false
            if (keystoreProps.isNotEmpty()) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    // AGP 9 内置 Kotlin 用这个 DSL 替代 kotlinOptions
    kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }
    buildFeatures { compose = true }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.09.00")
    implementation(composeBom)
    implementation("androidx.activity:activity-compose:1.9.2")
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.6")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
    // XML 主题（manifest 里引用的 Material3 主题在 material 库里）
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.compose.material:material-icons-extended")

    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")

    // 文件夹选择后的递归遍历（DocumentFile）
    implementation("androidx.documentfile:documentfile:1.0.1")

    // 相册选择器的缩略图加载
    implementation("io.coil-kt:coil-compose:2.7.0")

    // 二维码：生成（core）+ 扫码（embedded，内置 CaptureActivity）
    implementation("com.google.zxing:core:3.5.3")
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
}

// ---------------------------------------------------------------------------
// 构建产物自动归档：assembleRelease/assembleDebug 后自动复制——
//   app-release.apk → mobile/native-android/app-release.apk（仓库根副本，git 跟踪）
//   app-debug.apk   → mobile/native-android/app-debug.apk
//   app-release.apk → 仓库根 dist/LocalTransfer-<版本>-android-native.apk
// 版本读根 Cargo.toml（与 make-installer.ps1 / update-dist.ps1 的版本约定一致），
// 任何构建入口（命令行 gradle / Android Studio / CI）都生效。
// ---------------------------------------------------------------------------
val repoRoot = rootProject.projectDir.parentFile.parentFile   // mobile/native-android → 仓库根
val appVersion = File(repoRoot, "Cargo.toml").takeIf { it.exists() }
    ?.readText()?.lineSequence()
    ?.firstOrNull { it.trimStart().startsWith("version = ") }
    ?.substringAfter("\"")?.substringBefore("\"") ?: "0.0.0"

tasks.register("archiveApks") {
    group = "build"
    description = "归档 APK 到仓库根副本与 dist/"
    doLast {
        val outputsDir = File(layout.buildDirectory.get().asFile, "outputs/apk")
        val rel = File(outputsDir, "release/app-release.apk")
        val dbg = File(outputsDir, "debug/app-debug.apk")
        if (rel.exists()) {
            rel.copyTo(File(rootProject.projectDir, "app-release.apk"), overwrite = true)
            val distDir = File(repoRoot, "dist").apply { mkdirs() }
            rel.copyTo(File(distDir, "LocalTransfer-$appVersion-android-native.apk"),
                overwrite = true)
        }
        if (dbg.exists()) {
            dbg.copyTo(File(rootProject.projectDir, "app-debug.apk"), overwrite = true)
        }
        logger.lifecycle("APK 已归档（版本 $appVersion）：仓库根副本 + dist/")
    }
}

tasks.matching { it.name == "assembleRelease" || it.name == "assembleDebug" }.configureEach {
    finalizedBy("archiveApks")
}
