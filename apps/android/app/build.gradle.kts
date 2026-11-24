import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

val signingFile = file(
    providers.gradleProperty("kumowatch.signing").orNull
        ?: "${System.getProperty("user.home")}/.config/kumowatch/android-signing.properties"
)
val signing = Properties().apply { if (signingFile.exists()) signingFile.inputStream().use { load(it) } }
val buildMinutes = providers.gradleProperty("kumowatch.versionCode").map { it.toInt() }
    .orElse(providers.provider { ((System.currentTimeMillis() / 1000 - 1_767_225_600L) / 60).toInt() })
val appVersion = providers.gradleProperty("kumowatch.versionName").orElse("0.2.6")

android {
    namespace = "com.kravtsov.kumowatch"
    compileSdk = 37

    defaultConfig {
        applicationId = "com.kravtsov.kumowatch"
        minSdk = 26
        targetSdk = 36
        versionCode = buildMinutes.get()
        versionName = appVersion.get()
        manifestPlaceholders["appLabel"] = "Better Kumo"
    }

    signingConfigs {
        if (signing.containsKey("storeFile")) {
            create("release") {
                storeFile = file(signing.getProperty("storeFile"))
                storePassword = signing.getProperty("storePassword")
                keyAlias = signing.getProperty("keyAlias")
                keyPassword = signing.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
            versionNameSuffix = "-dev"
            manifestPlaceholders["appLabel"] = "Better Kumo Dev"
        }
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
            ndk {
                abiFilters += providers.gradleProperty("kumowatch.abis").orElse("arm64-v8a").get().split(",")
            }
        }
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.daily.client)
    implementation(libs.coil.compose)
    implementation(libs.coil.network.okhttp)
    implementation(libs.coil.gif)
    implementation("${libs.jna.get()}@aar")
}
