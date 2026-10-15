import org.gradle.api.tasks.Exec
import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// ---------------------------------------------------------------------------
// Rust cross-compile: cargo-ndk builds `myco-core` for arm64 and drops
// libmyco_core.so into jniLibs, which Gradle then packages. See
// docs/how-to/build.md.
// ---------------------------------------------------------------------------
val repoRoot = layout.projectDirectory.dir("../..")
val rustOutputDir = layout.projectDirectory.dir("src/main/jniLibs")

/** Short git commit the APK was built from, suffixed `-dirty` when the working
 *  tree has uncommitted edits. Surfaced in Settings via BuildConfig.GIT_REV. */
fun gitRev(): String = runCatching {
    fun run(vararg cmd: String) = ProcessBuilder(*cmd)
        .directory(repoRoot.asFile)
        .redirectErrorStream(true)
        .start().inputStream.bufferedReader().readText().trim()
    val hash = run("git", "rev-parse", "--short=8", "HEAD")
    val dirty = run("git", "status", "--porcelain").isNotEmpty()
    if (hash.isEmpty()) "unknown" else if (dirty) "$hash-dirty" else hash
}.getOrDefault("unknown")

/**
 * Wire in a LOCAL `fips` checkout via `patch.crates-io`. Set MYCO_FIPS_REPO_PATH
 * to the fips source tree (a single crate: Cargo.toml + src/lib.rs). See
 * docs/how-to/build.md §4.
 */
fun localFipsCargoConfigArgs(): List<String> {
    val fipsPath = System.getenv("MYCO_FIPS_REPO_PATH")?.takeIf { it.isNotBlank() }
        ?: return emptyList()
    val fipsRoot = file(fipsPath)
    require(fipsRoot.resolve("Cargo.toml").isFile && fipsRoot.resolve("src/lib.rs").isFile) {
        "MYCO_FIPS_REPO_PATH must point at a fips checkout (Cargo.toml + src/lib.rs)"
    }
    return listOf("--config", "patch.crates-io.fips.path=\"${fipsRoot.absolutePath}\"")
}

/**
 * Cargo features for `myco-core` that depend on what the local fips checkout
 * carries. `fips-multipath` (BLE as a backup path) needs the path roles of the
 * multi-path branch; detected from the checkout rather than asked for, so a
 * build against fips master — which has no roles — needs nothing set.
 */
fun mycoCoreFeatureArgs(): List<String> {
    val fipsPath = System.getenv("MYCO_FIPS_REPO_PATH")?.takeIf { it.isNotBlank() }
        ?: return emptyList()
    val transportConfig = file(fipsPath).resolve("src/config/transport.rs")
    val multipath = transportConfig.isFile && transportConfig.readText().contains("pub enum TransportRole")
    return if (multipath) listOf("--features", "fips-multipath") else emptyList()
}

/**
 * ABIs to build and package. arm64-v8a by default; Android TV boxes often run
 * a 32-bit userspace on 64-bit chips, so a TV build adds armeabi-v7a:
 * `./gradlew assembleDebug -Pmyco.abis=arm64-v8a,armeabi-v7a` (or MYCO_ABIS).
 */
val mycoAbis: List<String> =
    ((findProperty("myco.abis") as String?) ?: System.getenv("MYCO_ABIS") ?: "arm64-v8a")
        .split(',').map { it.trim() }.filter { it.isNotEmpty() }

tasks.register<Exec>("buildRust") {
    workingDir = repoRoot.asFile
    commandLine(
        *(listOf("cargo", "ndk")
          + mycoAbis.flatMap { listOf("--target", it) }
          + listOf(
            "--platform", "29",                       // minSdk 29 (L2CAP CoC)
            "--output-dir", rustOutputDir.asFile.absolutePath,
            "build",
        ) + localFipsCargoConfigArgs()
          + listOf("--package", "myco-core", "--release")
          + mycoCoreFeatureArgs()
        ).toTypedArray()
    )
}

tasks.matching { it.name in listOf("mergeDebugNativeLibs", "mergeReleaseNativeLibs", "mergeTvNativeLibs") }
    .configureEach { dependsOn("buildRust") }

// Release signing. Credentials live OUTSIDE git: an `android/keystore.properties`
// file (gitignored) or environment variables (CI / an externally-managed or
// remote keystore). When neither is present the release build is left unsigned
// so dev/debug builds keep working — the keystore is supplied only at publish
// time. See docs/how-to/publish.md.
val keystorePropsFile = rootProject.file("keystore.properties")
val keystoreProps = Properties().apply {
    if (keystorePropsFile.exists()) keystorePropsFile.inputStream().use { load(it) }
}
fun signingProp(key: String, env: String): String? =
    keystoreProps.getProperty(key) ?: System.getenv(env)
val hasReleaseKeystore = signingProp("storeFile", "MYCO_RELEASE_STORE_FILE") != null

android {
    namespace = "app.myco"
    compileSdk = 36

    defaultConfig {
        applicationId = "app.myco"
        minSdk = 29                                   // L2CAP CoC requirement
        targetSdk = 36
        // CI overrides these from the release tag (see .github/workflows/release.yml)
        // so each store release gets a unique, increasing versionCode; local
        // builds fall back to the committed defaults.
        versionCode = System.getenv("MYCO_VERSION_CODE")?.toIntOrNull() ?: 1
        versionName = System.getenv("MYCO_VERSION_NAME") ?: "0.0.1"
        ndk { abiFilters += mycoAbis }                // see mycoAbis
        buildConfigField("String", "GIT_REV", "\"${gitRev()}\"")
    }

    signingConfigs {
        create("release") {
            if (hasReleaseKeystore) {
                storeFile = file(signingProp("storeFile", "MYCO_RELEASE_STORE_FILE")!!)
                storePassword = signingProp("storePassword", "MYCO_RELEASE_STORE_PASSWORD")
                keyAlias = signingProp("keyAlias", "MYCO_RELEASE_KEY_ALIAS")
                keyPassword = signingProp("keyPassword", "MYCO_RELEASE_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            // R8 drops unused Kotlin/Java code and resources; without it the
            // dex alone was ~43 MB (mostly material-icons-extended). The keep
            // rules for what Rust reaches over JNI are in proguard-rules.pro.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // Signed only when a keystore is configured; otherwise an unsigned
            // release APK is produced (zsp / CI signs at publish time).
            signingConfig = if (hasReleaseKeystore)
                signingConfigs.getByName("release") else null
        }
        // Release code (R8, not debuggable) signed with the debug key, so it
        // installs over a debug build on a test device. Debug Compose is
        // several times slower than release, which on a weak Android TV is
        // the difference between usable and not: measure performance here.
        //   ./gradlew assembleTv -Pmyco.abis=armeabi-v7a
        create("tv") {
            initWith(getByName("release"))
            signingConfig = signingConfigs.getByName("debug")
            isDebuggable = false
            matchingFallbacks += "release"
        }
    }

    buildFeatures { compose = true; buildConfig = true }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
}

// The core's state model is immutable data classes behind `List`/`Map`/`Set`
// interfaces, which the Compose compiler can't prove stable on its own. Without
// this, every composable taking `AppState` or a list from it is compared by
// instance and recomposes on every poll even when nothing it draws changed.
composeCompiler {
    stabilityConfigurationFiles.add(layout.projectDirectory.file("compose-stability.conf"))
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    // System splash (Android 12+ API, backported to minSdk via the library): a
    // black window with the Myco mark while the activity warms up.
    implementation("androidx.core:core-splashscreen:1.0.1")
    // The origin-scoped WebView message channel the napplet runtime uses.
    // `addWebMessageListener` injects only into frames matching an explicit
    // origin rule, unlike `addJavascriptInterface`, whose object lands in
    // every frame including a sandboxed napplet's.
    implementation("androidx.webkit:webkit:1.12.1")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation(platform("androidx.compose:compose-bom:2024.10.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    // ProcessLifecycleOwner: app-visibility signal driving background BLE duty cycle.
    implementation("androidx.lifecycle:lifecycle-process:2.8.7")
    // Bottom-nav shell (Apps · Circle · Settings · Dev). minSdk-29 safe.
    implementation("androidx.navigation:navigation-compose:2.8.4")
    // QR generation for share-an-nsite (encodes the nsite id + pairing info)…
    implementation("com.google.zxing:core:3.5.3")
    // Embedded HTTP server for the file-share hotspot's upload/download page.
    implementation("org.nanohttpd:nanohttpd:2.3.1")
    // …and an in-app camera scanner to read one back.
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    testImplementation("junit:junit:4.13.2")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
