plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.protobuf)
}

// Configuration for Chicory build-time WASM compilation
val chicoryCompiler: Configuration by configurations.creating {
    isTransitive = false
}

// Task to compile WASM to JVM bytecode at build time
val compileWasm by tasks.registering {
    group = "build"
    description = "Compiles WASM to JVM bytecode using Chicory build-time compiler"

    val wasmFile = file("src/main/resources/wasm/confidence_resolver.wasm")
    val outputSourceDir = layout.buildDirectory.dir("generated/wasm-sources/java")
    val outputClassDir = layout.buildDirectory.dir("generated/wasm-classes")
    val outputMetaDir = layout.buildDirectory.dir("generated/wasm-meta")

    inputs.file(wasmFile)
    inputs.files(chicoryCompiler)
    outputs.dir(outputSourceDir)
    outputs.dir(outputClassDir)
    outputs.dir(outputMetaDir)

    doLast {
        outputSourceDir.get().asFile.mkdirs()
        outputClassDir.get().asFile.mkdirs()
        outputMetaDir.get().asFile.mkdirs()

        val cliJar = chicoryCompiler.singleFile
        exec {
            commandLine(
                "java", "-jar",
                cliJar.absolutePath,
                "--source-dir=${outputSourceDir.get().asFile.absolutePath}",
                "--class-dir=${outputClassDir.get().asFile.absolutePath}",
                "--wasm-dir=${outputMetaDir.get().asFile.absolutePath}",
                "--prefix=com.spotify.confidence.wasm.ConfidenceResolver",
                "--interpreter-fallback=WARN",
                wasmFile.absolutePath
            )
        }
    }
}

android {
    namespace = "com.spotify.confidence.android"
    compileSdk = 34

    defaultConfig {
        minSdk = 26

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        consumerProguardFiles("consumer-rules.pro")
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

    packaging {
        resources {
            // Exclude proto files that conflict with protobuf-javalite
            excludes += "google/protobuf/*.proto"
            excludes += "google/api/*.proto"
            excludes += "google/type/*.proto"
        }
    }

    sourceSets {
        getByName("main") {
            resources.srcDirs("src/main/resources")
            // Include pre-compiled WASM meta file
            resources.srcDirs(layout.buildDirectory.dir("generated/wasm-meta"))
            // Include pre-compiled WASM Java source
            java.srcDirs(layout.buildDirectory.dir("generated/wasm-sources/java"))
        }
    }
}

// Wire WASM compilation into the build
tasks.named("preBuild") {
    dependsOn("compileWasm")
}

// Add pre-compiled WASM classes to the compile classpath and bundle
android.libraryVariants.all {
    val variant = this
    val variantName = variant.name.replaceFirstChar { it.uppercaseChar() }

    // Add to compile classpath
    val compileTask = tasks.named<org.gradle.api.tasks.compile.JavaCompile>("compile${variantName}JavaWithJavac")
    compileTask.configure {
        dependsOn("compileWasm")
        classpath += files(layout.buildDirectory.dir("generated/wasm-classes"))
    }

    // Register the pre-compiled classes as additional class files
    variant.registerPostJavacGeneratedBytecode(files(layout.buildDirectory.dir("generated/wasm-classes")))
}

protobuf {
    protoc {
        artifact = "com.google.protobuf:protoc:${libs.versions.protobuf.get()}"
    }
    plugins {
        create("grpc") {
            artifact = "io.grpc:protoc-gen-grpc-java:${libs.versions.grpc.get()}"
        }
    }
    generateProtoTasks {
        all().forEach { task ->
            task.builtins {
                create("java") {
                    option("lite")
                }
            }
            task.plugins {
                create("grpc") {
                    option("lite")
                }
            }
        }
    }
}


dependencies {
    // Chicory build-time WASM compiler (CLI tool)
    chicoryCompiler("com.dylibso.chicory:build-time-compiler-cli-experimental:${libs.versions.chicory.get()}")

    // OpenFeature SDK for Android
    implementation(libs.openfeature.android)

    // WASM runtime - Chicory
    implementation(libs.bundles.chicory)

    // Protobuf
    implementation(libs.protobuf.javalite)

    // gRPC for Android
    implementation(libs.bundles.grpc)

    // javax.annotation for gRPC generated code
    compileOnly("javax.annotation:javax.annotation-api:1.3.2")

    // Networking
    implementation(libs.okhttp)

    // Coroutines
    implementation(libs.bundles.coroutines)

    // Testing
    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
    testImplementation(libs.mockk)
}
