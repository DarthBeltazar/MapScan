plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "com.orienteering.mapapp.app"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = "com.orienteering.mapapp.app"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName

        // Restrict to the two ABIs actually verified against OpenCV
        // (arm64-v8a: real devices; x86_64: desktop emulators). Without this,
        // a stale armeabi-v7a librust_core.so left over from before the CV
        // pipeline/OpenCV were added silently ends up in the APK - cargokit
        // only rebuilds targets Gradle actually asks for, and armeabi-v7a
        // wasn't among them, but the merge step still picked up the old
        // artifact sitting in that target's build dir. 32-bit x86 dropped too
        // since it's essentially unused (no OpenCV Android SDK verification
        // done against it either).
        ndk {
            abiFilters += setOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            // TODO: Add your own signing config for the release build.
            // Signing with the debug keys for now, so `flutter run --release` works.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    // rust_core.so (built by cargokit) dynamically links against
    // libopencv_java4.so rather than statically bundling OpenCV - that shared
    // lib needs to ship in the APK too, or the app fails to load rust_core.so
    // at runtime with UnsatisfiedLinkError. The OpenCV Android SDK's
    // sdk/native/libs/<abi>/libopencv_java4.so layout matches Gradle's
    // jniLibs.srcDirs convention directly (one subdir per ABI).
    val openCvAndroidSdkPath = System.getenv("OPENCV_ANDROID_SDK_PATH")
    if (openCvAndroidSdkPath != null) {
        sourceSets {
            getByName("main") {
                jniLibs.srcDirs("$openCvAndroidSdkPath/sdk/native/libs")
            }
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}
