import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// The Data Layer connects the two apps only with the same application ID and signing certificate.
// Paths resolve as the phone app's build resolves them.
val appDir = rootProject.file("app")
fun appFile(path: String): File = project(":app").file(path)
val signingFile = appFile(
    providers.gradleProperty("kumowatch.signing").orNull
        ?: "${System.getProperty("user.home")}/.config/kumowatch/android-signing.properties"
)
val signing = Properties().apply { if (signingFile.exists()) signingFile.inputStream().use { load(it) } }
// One version for both apps; the release flow bumps it in the phone app's build file.
val appVersion = providers.gradleProperty("kumowatch.versionName").orElse(providers.provider {
    Regex("""kumowatch\.versionName"\)\s*\.orElse\(\s*"([^"]+)"""")
        .find(appDir.resolve("build.gradle.kts").readText())?.groupValues?.get(1)
        ?: error("No kumowatch.versionName default in app/build.gradle.kts; pass -Pkumowatch.versionName")
})
// From the version, so re-running an older tag never outranks a newer release on Play. 10 000 000 and up stays above
// the build-minute codes earlier releases used.
val versionCodeFromName = providers.gradleProperty("kumowatch.versionCode").map { it.toInt() }.orElse(appVersion.map { name ->
    val (major, minor, patch) = Regex("""^(\d+)\.(\d+)\.(\d+)""").find(name)?.destructured
        ?: error("versionName $name isn't major.minor.patch")
    require(minor.toInt() < 100 && patch.toInt() < 100) { "versionName $name: minor and patch must stay below 100" }
    10_000_000 + major.toInt() * 1_000_000 + minor.toInt() * 10_000 + patch.toInt() * 100
})
val wearAbis = providers.gradleProperty("kumowatch.wearAbis").orElse("arm64-v8a,armeabi-v7a").get().split(",")

android {
    namespace = "com.kravtsov.kumowatch.wear"
    compileSdk = 37

    defaultConfig {
        applicationId = "com.kravtsov.kumowatch"
        minSdk = 30
        targetSdk = 36
        // One Play listing holds both apps, and every bundle in it needs its own code.
        versionCode = 1_000_000_000 + versionCodeFromName.get()
        versionName = appVersion.get()
        manifestPlaceholders["appLabel"] = "Better Kumo"
    }

    signingConfigs {
        if (signing.containsKey("storeFile")) {
            create("release") {
                storeFile = appFile(signing.getProperty("storeFile"))
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
            // Only the ABIs the last core build made: JNA alone would install and then crash.
            ndk {
                abiFilters += appDir.resolve("src/main/jniLibs").listFiles { dir -> dir.resolve("libkumo_ffi.so").isFile }
                    ?.map { it.name }.orEmpty()
            }
        }
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
            ndk {
                abiFilters += wearAbis
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

// JNA ships every ABI, so a missing Rust core would only show as a crash on the watch.
tasks.matching { it.name == "preReleaseBuild" }.configureEach {
    val abis = wearAbis
    val jniLibs = appDir.resolve("src/main/jniLibs")
    doFirst {
        val missing = abis.filterNot { jniLibs.resolve("$it/libkumo_ffi.so").isFile }
        if (missing.isNotEmpty()) throw GradleException("No Rust core for $missing: run scripts/build-core-android.sh with those ABIS")
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
