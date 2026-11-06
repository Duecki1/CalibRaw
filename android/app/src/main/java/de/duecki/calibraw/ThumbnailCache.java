package de.duecki.calibraw;

import android.app.Activity;
import android.util.Log;

import java.io.File;
import java.util.Arrays;
import java.util.HashMap;
import java.util.Map;
import java.util.concurrent.atomic.AtomicInteger;

final class ThumbnailCache {
    private static final String LOG_TAG = "CalibRaw";
    private static final long MAX_BYTES = 5L * 1024L * 1024L * 1024L;
    private static final int TRIM_INTERVAL = 64;
    private static final int DELETE_ATTEMPTS = 3;

    private final Activity storage;
    private final AtomicInteger maintainCalls = new AtomicInteger();

    ThumbnailCache(Activity storage) {
        this.storage = storage;
    }

    /**
     * The cache directory. Native code names, reads and writes the files in it
     * (calibraw-ffi {@code android/thumbnail_cache.rs}): a ".jpg" with an optional
     * ".fingerprint" beside it, and per-folder metadata. Trimming here takes a file's
     * modification time as its last use, which native code updates on every read.
     */
    File directory() {
        return persistentDirectory();
    }

    void clear() {
        try {
            clearDirectory(persistentDirectory());
        } catch (RuntimeException error) {
            Log.w(LOG_TAG, "Could not clear thumbnail cache", error);
        }
    }

    long sizeBytes() {
        return directorySize(persistentDirectory());
    }

    void maintain() {
        // Scanning a large cache is too slow for every saved thumbnail. The first call trims;
        // what accumulates between trims is negligible against MAX_BYTES.
        if (maintainCalls.getAndIncrement() % TRIM_INTERVAL != 0) {
            return;
        }
        try {
            trim(persistentDirectory());
        } catch (RuntimeException error) {
            Log.w(LOG_TAG, "Could not maintain thumbnail cache", error);
        }
    }

    // Bounded no-backup storage survives cache eviction but is removed on uninstall.
    private File persistentDirectory() {
        File directory = new File(storage.getNoBackupFilesDir(), "library-thumbnails");
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IllegalStateException("Could not create the persistent thumbnail cache");
        }
        return directory;
    }

    private static void clearDirectory(File directory) {
        if (!directory.exists()) {
            return;
        }
        File[] entries = directory.listFiles();
        if (entries == null) {
            Log.w(LOG_TAG, "Could not inspect thumbnail cache " + directory);
            return;
        }
        for (File entry : entries) {
            if (!entry.isFile()) {
                Log.w(LOG_TAG, "Skipping unexpected thumbnail-cache entry " + entry);
                continue;
            }
            deleteFile(entry);
        }
    }

    private static long directorySize(File directory) {
        if (!directory.isDirectory()) {
            return 0L;
        }
        File[] entries = directory.listFiles();
        if (entries == null) {
            return 0L;
        }
        long total = 0L;
        for (File entry : entries) {
            long bytes = entry.isDirectory() ? directorySize(entry) : Math.max(0L, entry.length());
            total = total > Long.MAX_VALUE - bytes ? Long.MAX_VALUE : total + bytes;
        }
        return total;
    }

    static void trim(File directory) {
        File[] legacyPngs = directory.listFiles(
                (parent, name) -> name.endsWith(".png") || name.endsWith(".png.fingerprint"));
        if (legacyPngs != null) {
            for (File legacyPng : legacyPngs) {
                deleteFile(legacyPng);
            }
        }

        File[] fingerprints = directory.listFiles(
                (parent, name) -> name.endsWith(".developed.jpg.fingerprint"));
        if (fingerprints != null) {
            for (File fingerprint : fingerprints) {
                String name = fingerprint.getName();
                String thumbnailName = name.substring(0, name.length() - ".fingerprint".length());
                if (!new File(directory, thumbnailName).isFile()) {
                    deleteFile(fingerprint);
                }
            }
        }

        File[] thumbnails = directory.listFiles((parent, name) -> name.endsWith(".jpg"));
        if (thumbnails == null || thumbnails.length == 0) {
            return;
        }
        long totalBytes = directorySize(directory);
        if (totalBytes <= MAX_BYTES) {
            return;
        }

        // Snapshot the times: touch() runs concurrently, and a sort key that changes mid-sort
        // can make the sort throw, which would leave the cache untrimmed.
        Map<File, Long> lastUsed = new HashMap<>(thumbnails.length);
        for (File thumbnail : thumbnails) {
            lastUsed.put(thumbnail, thumbnail.lastModified());
        }
        Arrays.sort(
                thumbnails,
                (left, right) -> Long.compare(lastUsed.get(left), lastUsed.get(right)));

        for (File thumbnail : thumbnails) {
            if (totalBytes <= MAX_BYTES) {
                break;
            }
            File fingerprint = new File(thumbnail.getPath() + ".fingerprint");
            long removedBytes = Math.max(0L, thumbnail.length())
                    + Math.max(0L, fingerprint.length());
            deleteEntry(thumbnail);
            if (!thumbnail.exists() && !fingerprint.exists()) {
                totalBytes = Math.max(0L, totalBytes - removedBytes);
            } else {
                totalBytes = directorySize(directory);
            }
        }
    }

    private static void deleteEntry(File thumbnail) {
        deleteFile(thumbnail);
        deleteFile(new File(thumbnail.getPath() + ".fingerprint"));
    }

    private static void deleteFile(File file) {
        try {
            for (int attempt = 1; attempt <= DELETE_ATTEMPTS; attempt++) {
                if (!file.exists() || file.delete()) {
                    return;
                }
                if (attempt < DELETE_ATTEMPTS) {
                    Thread.yield();
                }
            }
            Log.w(
                    LOG_TAG,
                    "Could not delete thumbnail-cache file after " + DELETE_ATTEMPTS
                            + " attempts; leaving it in place: " + file);
        } catch (RuntimeException error) {
            Log.w(LOG_TAG, "Could not delete thumbnail-cache file " + file, error);
        }
    }
}
