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

tasks.register<Exec>("buildRustArm64") {
    workingDir = repoRoot.asFile
    commandLine(
        *(listOf(
            "cargo", "ndk",
            "--target", "arm64-v8a",
            "--platform", "29",                       // minSdk 29 (L2CAP CoC)
            "--output-dir", rustOutputDir.asFile.absolutePath,
            "build",
        ) + localFipsCargoConfigArgs()
          + listOf("--package", "myco-core", "--release")
        ).toTypedArray()
    )
}

tasks.matching { it.name in listOf("mergeDebugNativeLibs", "mergeReleaseNativeLibs") }
    .configureEach { dependsOn("buildRustArm64") }

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
        ndk { abiFilters += "arm64-v8a" }             // arm64 only
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
            isMinifyEnabled = false
            // Signed only when a keystore is configured; otherwise an unsigned
            // release APK is produced (zsp / CI signs at publish time).
            signingConfig = if (hasReleaseKeystore)
                signingConfigs.getByName("release") else null
        }
    }

    buildFeatures { compose = true; buildConfig = true }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    // System splash (Android 12+ API, backported to minSdk via the library): a
    // black window with the Myco mark while the activity warms up.
    implementation("androidx.core:core-splashscreen:1.0.1")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation(platform("androidx.compose:compose-bom:2024.10.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    // ProcessLifecycleOwner: app-visibility signal driving background BLE duty cycle.
    implementation("androidx.lifecycle:lifecycle-process:2.8.7")
    // Bottom-nav shell (Apps · Circle · Discover · Settings · Dev). minSdk-29 safe.
    implementation("androidx.navigation:navigation-compose:2.8.4")
    // QR generation for share-an-nsite (encodes the nsite id + pairing info)…
    implementation("com.google.zxing:core:3.5.3")
    // …and an in-app camera scanner to read one back.
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
