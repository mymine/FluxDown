plugins {
    alias(libs.plugins.android.library)
}

// :core —— 主机契约（HostSession，由 :bridge 以 UniFFI `native/mobile` 实现）+ 快照/事件状态仓库 + 纯函数格式化。
// 不依赖 Compose / UI；JVM 单测覆盖状态迁移。
android {
    namespace = "com.fluxdown.core"
    compileSdk {
        version = release(37)
    }

    defaultConfig {
        minSdk = 26
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    api(libs.kotlinx.coroutines.core)
    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}
