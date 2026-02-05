import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

// Load local.properties for secrets
val localProperties = Properties().apply {
    val localPropertiesFile = rootProject.file("local.properties")
    if (localPropertiesFile.exists()) {
        load(localPropertiesFile.inputStream())
    }
}

android {
    namespace = "com.spotify.confidence.demo"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.spotify.confidence.demo"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "1.0"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"

        // Read client secret from local.properties
        buildConfigField(
            "String",
            "CONFIDENCE_CLIENT_SECRET",
            "\"${localProperties.getProperty("confidence.clientSecret", "")}\""
        )
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        viewBinding = true
        buildConfig = true
    }

    packaging {
        resources {
            // Exclude proto files that conflict with protobuf-javalite
            excludes += "google/protobuf/*.proto"
            excludes += "google/api/*.proto"
            excludes += "google/type/*.proto"
        }
    }
}

dependencies {
    implementation(project(":confidence-provider"))

    // OpenFeature SDK for Android
    implementation(libs.openfeature.android)

    // Android core
    implementation("androidx.core:core-ktx:1.12.0")
    implementation("androidx.appcompat:appcompat:1.6.1")
    implementation("com.google.android.material:material:1.11.0")
    implementation("androidx.constraintlayout:constraintlayout:2.1.4")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.7.0")

    // Coroutines
    implementation(libs.bundles.coroutines)
}
