plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "com.lunote.lunote_app"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = "28.2.13676358"

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = "com.lunote.lunote_app"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        // Uses the version code from pubspec.yaml. When using split APKs, 1000 * ABI_VERSION
        // is added automatically by Flutter. (https://developer.android.com/studio/build/configure-apk-splits#configure-APK-versions)
        // You can force using the value of versionCode by specifying the `-P force-version-code-ignoring-abi=true`
        // flag during build.
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    buildTypes {
        release {
            // Preserve the existing publishing certificate for in-place updates.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    // Release builds must explicitly point at the existing publishing key.
    val publishingKey = System.getenv("LUNOTE_SIGNING_KEYSTORE")
    if (!publishingKey.isNullOrBlank()) {
        signingConfigs.getByName("debug").storeFile = file(publishingKey)
    }
    gradle.taskGraph.whenReady {
        if (allTasks.any { it.project == project && it.name in listOf("assembleRelease", "bundleRelease") }
            && publishingKey.isNullOrBlank()) {
            throw GradleException("Set LUNOTE_SIGNING_KEYSTORE to the existing publishing key before a release build")
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
