package app.myco.share

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MycoApkTest {

    @Test
    fun copyIsFreshOnlyWhenSameSizeAndWrittenAfterTheInstall() {
        assertTrue(MycoApk.isFreshCopy(copyLen = 100, copyModified = 2_000, apkLen = 100, apkModified = 1_000))
        // A new build of the same size, installed after the copy was made.
        assertFalse(MycoApk.isFreshCopy(copyLen = 100, copyModified = 1_000, apkLen = 100, apkModified = 2_000))
        // A half-written or different file.
        assertFalse(MycoApk.isFreshCopy(copyLen = 50, copyModified = 2_000, apkLen = 100, apkModified = 1_000))
    }

    @Test
    fun only64BitWhenEveryNativeLibraryIs() {
        assertTrue(MycoApk.only64Bit(setOf("arm64-v8a")))
        assertTrue(MycoApk.only64Bit(setOf("arm64-v8a", "x86_64")))
        // A 32-bit TV build, or one carrying both, runs on 32-bit phones too.
        assertFalse(MycoApk.only64Bit(setOf("armeabi-v7a")))
        assertFalse(MycoApk.only64Bit(setOf("arm64-v8a", "armeabi-v7a")))
        // No native code at all: nothing ties it to 64-bit.
        assertFalse(MycoApk.only64Bit(emptySet()))
    }
}
