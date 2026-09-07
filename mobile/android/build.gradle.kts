allprojects {
    repositories {
        // 国内镜像优先，官方源兜底
        maven { url = uri("https://maven.aliyun.com/repository/google") }
        maven { url = uri("https://maven.aliyun.com/repository/public") }
        google()
        mavenCentral()
    }
}

val newBuildDir: Directory =
    rootProject.layout.buildDirectory
        .dir("../../build")
        .get()
rootProject.layout.buildDirectory.value(newBuildDir)

subprojects {
    val newSubprojectBuildDir: Directory = newBuildDir.dir(project.name)
    project.layout.buildDirectory.value(newSubprojectBuildDir)
}
// 部分插件（如 file_picker 8.x）自带 compileSdk 34，与新版
// flutter_plugin_android_lifecycle（要求 36+）冲突——统一覆盖为 36。
// 注意：afterEvaluate 必须在 evaluationDependsOn 之前注册。
subprojects {
    afterEvaluate {
        val androidExt = extensions.findByName("android") ?: return@afterEvaluate
        for (name in listOf("setCompileSdkVersion", "setCompileSdk")) {
            try {
                val m = androidExt.javaClass.getMethod(name, Int::class.javaPrimitiveType)
                m.invoke(androidExt, 36)
                break
            } catch (_: NoSuchMethodException) {
            } catch (_: Exception) {
            }
        }
    }
    project.evaluationDependsOn(":app")
}

tasks.register<Delete>("clean") {
    delete(rootProject.layout.buildDirectory)
}
