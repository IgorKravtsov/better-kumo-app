import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// The Data Layer connects the two apps only with the same application ID and signing certificate.
val signingFile = file(
    providers.gradleProperty("kumowatch.signing").orNull
        ?: "${System.getProperty("user.home")}/.config/kumowatch/android-signing.properties"
)
val signing = Properties().apply { if (signingFile.exists()) signingFile.inputStream().use { load(it) } }
val buildMinutes = providers.gradleProperty("kumowatch.versionCode").map { it.toInt() }
    .orElse(providers.provider { ((System.currentTimeMillis() / 1000 - 1_767_225_600L) / 60).toInt() })
// One version for both apps; the release flow bumps it in the phone app's build file.
val phoneVersion = Regex("""kumowatch\.versionName"\)\.orElse\("([^"]+)"\)""")
    .find(file("../app/build.gradle.kts").readText())!!.groupValues[1]
val appVersion = providers.gradleProperty("kumowatch.versionName").orElse(phoneVersion)

android {
    namespace = "com.kravtsov.kumowatch.wear"
    compileSdk = 37

    defaultConfig {
        applicationId = "com.kravtsov.kumowatch"
        minSdk = 30
        targetSdk = 36
        // One Play listing holds both apps, and every bundle in it needs its own code.
        versionCode = 1_000_000_000 + buildMinutes.get()
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
                abiFilters += providers.gradleProperty("kumowatch.wearAbis").orElse("arm64-v8a,armeabi-v7a").get().split(",")
            }
        }
    }

    sourceSets["main"].kotlin.directories += listOf("../app/src/main/java/uniffi", "../common/java")
    sourceSets["main"].jniLibs.srcDir("../app/src/main/jniLibs")

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
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.material.icons)
    implementation(libs.wear.compose.material3)
    implementation(libs.wear.compose.foundation)
    implementation(libs.wear.compose.navigation)
    implementation(libs.wear.input)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.kotlinx.coroutines.play.services)
    implementation(libs.play.services.wearable)
    implementation(libs.daily.client)
    implementation("${libs.jna.get()}@aar")
}
