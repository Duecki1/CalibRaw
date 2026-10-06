package de.duecki.calibraw;

import android.Manifest;
import android.content.ContentResolver;
import android.content.ContentValues;
import android.content.pm.PackageManager;
import android.database.Cursor;
import android.media.MediaScannerConnection;
import android.net.Uri;
import android.os.Build;
import android.os.Environment;
import android.os.ParcelFileDescriptor;
import android.provider.MediaStore;
import android.util.Log;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.concurrent.atomic.AtomicReference;

final class ExportPublisher {
    static final int WRITE_EXPORT_PERMISSION = 1002;
    private static final String LOG_TAG = "CalibRaw";
    private static final int DELETE_ATTEMPTS = 3;
    private static final long STALE_EXPORT_CACHE_AGE_MS = 24L * 60L * 60L * 1000L;
    private static String exportDirectory(String mimeType) {
        return "video/mp4".equals(mimeType)
                ? Environment.DIRECTORY_MOVIES : Environment.DIRECTORY_PICTURES;
    }

    interface Callbacks {
        void onExportPublished(String location, String error);
    }

    private final CalibRawActivity activity;
    private final Callbacks callbacks;
    private final AtomicReference<PendingLegacyExport> pendingLegacyExport = new AtomicReference<>();

    ExportPublisher(CalibRawActivity activity, Callbacks callbacks) {
        this.activity = activity;
        this.callbacks = callbacks;
    }

    String createPendingExport(String requestedName, String mimeType) throws Exception {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return "";
        }
        String normalizedMime = AndroidStorageContract.normalizeExportMimeType(mimeType);
        String displayName = AndroidStorageContract.safeImageName(requestedName, normalizedMime);
        ContentResolver resolver = activity.getContentResolver();
        Uri uri = createPendingImage(displayName, normalizedMime);
        boolean transferred = false;
        try {
            ParcelFileDescriptor descriptor = resolver.openFileDescriptor(uri, "w");
            int fd = NativeFileDescriptors.detach(
                    descriptor, "Android MediaStore returned no file descriptor");
            transferred = true;
            String location = AndroidStorageContract.exportLocation(
                    exportDirectory(normalizedMime), displayName);
            return fd + "\t" + uri + "\t" + location;
        } finally {
            if (!transferred) {
                resolver.delete(uri, null, null);
            }
        }
    }

    /**
     * Publishes or deletes a pending export. A published export returns where MediaStore put it,
     * which carries a numbered name when the requested one was taken, or "" when MediaStore
     * cannot tell.
     */
    String finishPendingExport(String uriText, int successFlag) throws Exception {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || uriText == null || uriText.isEmpty()) {
            return "";
        }
        ContentResolver resolver = activity.getContentResolver();
        Uri uri = Uri.parse(uriText);
        boolean success = successFlag != 0;
        if (!success) {
            resolver.delete(uri, null, null);
            return "";
        }
        ContentValues values = new ContentValues();
        values.put(MediaStore.Images.Media.IS_PENDING, 0);
        if (resolver.update(uri, values, null, null) <= 0) {
            resolver.delete(uri, null, null);
            throw new IllegalStateException("Android MediaStore could not publish the export");
        }
        return publishedLocation(uri);
    }

    /**
     * The folder and name MediaStore gave a published item, or "" when it cannot be read.
     * MediaStore numbers a name that is already taken, so this can differ from the requested one.
     */
    private String publishedLocation(Uri uri) {
        String[] projection = {
                MediaStore.MediaColumns.RELATIVE_PATH, MediaStore.MediaColumns.DISPLAY_NAME};
        try (Cursor cursor = activity.getContentResolver().query(
                uri, projection, null, null, null)) {
            if (cursor == null || !cursor.moveToFirst()) {
                return "";
            }
            return AndroidStorageContract.mediaStoreLocation(
                    cursor.getString(0), cursor.getString(1));
        } catch (RuntimeException error) {
            Log.w(LOG_TAG, "Could not read the published export name", error);
            return "";
        }
    }

    void publishImage(String cachedPath, String displayName, String mimeType) {
        activity.runOnUiThread(() -> beginPublishImage(cachedPath, displayName, mimeType));
    }

    private void beginPublishImage(String cachedPath, String displayName, String mimeType) {
        String normalizedMime = AndroidStorageContract.normalizeExportMimeType(mimeType);
        if (Build.VERSION.SDK_INT <= Build.VERSION_CODES.P
                && activity.checkSelfPermission(Manifest.permission.WRITE_EXTERNAL_STORAGE)
                != PackageManager.PERMISSION_GRANTED) {
            PendingLegacyExport replaced = pendingLegacyExport.getAndSet(
                    new PendingLegacyExport(cachedPath, displayName, normalizedMime));
            if (replaced != null) {
                deleteCachedExport(replaced.cachedPath);
                callbacks.onExportPublished(
                        "", "A newer export replaced the pending permission request");
            }
            activity.requestPermissions(
                    new String[]{Manifest.permission.WRITE_EXTERNAL_STORAGE},
                    WRITE_EXPORT_PERMISSION);
            return;
        }
        startPublishThread(cachedPath, displayName, normalizedMime);
    }


    private void startPublishThread(String cachedPath, String displayName, String mimeType) {
        new Thread(
                () -> publishImageInBackground(cachedPath, displayName, mimeType),
                "CalibRaw image publish").start();
    }

    private void publishImageInBackground(
            String cachedPath,
            String requestedName,
            String mimeType) {
        File cachedFile = new File(cachedPath);
        String normalizedMime = AndroidStorageContract.normalizeExportMimeType(mimeType);
        String displayName = AndroidStorageContract.safeImageName(requestedName, normalizedMime);
        try {
            String location;
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                location = publishImageScoped(cachedFile, displayName, normalizedMime);
            } else {
                location = publishImageLegacy(cachedFile, displayName, normalizedMime);
            }
            callbacks.onExportPublished(location, "");
        } catch (Exception error) {
            callbacks.onExportPublished("", error.toString());
        } finally {
            deleteCachedExport(cachedFile);
        }
    }

    private Uri createPendingImage(String displayName, String mimeType) {
        ContentValues values = new ContentValues();
        values.put(MediaStore.Images.Media.DISPLAY_NAME, displayName);
        values.put(MediaStore.Images.Media.MIME_TYPE, mimeType);
        values.put(
                MediaStore.Images.Media.RELATIVE_PATH,
                AndroidStorageContract.exportRelativePath(exportDirectory(mimeType)));
        values.put(MediaStore.Images.Media.IS_PENDING, 1);

        Uri uri = activity.getContentResolver().insert(
                "video/mp4".equals(mimeType)
                        ? MediaStore.Video.Media.EXTERNAL_CONTENT_URI
                        : MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values);
        if (uri == null) {
            throw new IllegalStateException("Android MediaStore could not create the export");
        }
        return uri;
    }

    private String publishImageScoped(
            File cachedFile,
            String displayName,
            String mimeType) throws Exception {
        ContentResolver resolver = activity.getContentResolver();
        Uri uri = createPendingImage(displayName, mimeType);
        boolean published = false;
        try {
            try (InputStream input = new FileInputStream(cachedFile);
                 OutputStream output = resolver.openOutputStream(uri, "w")) {
                if (output == null) {
                    throw new IllegalStateException("Android MediaStore returned no output stream");
                }
                BoundedStreams.copy(input, output, Long.MAX_VALUE, "Export is too large");
            }
            ContentValues values = new ContentValues();
            values.put(MediaStore.Images.Media.IS_PENDING, 0);
            if (resolver.update(uri, values, null, null) <= 0) {
                throw new IllegalStateException("Android MediaStore could not publish the export");
            }
            published = true;
            String location = publishedLocation(uri);
            return location.isEmpty()
                    ? AndroidStorageContract.exportLocation(exportDirectory(mimeType), displayName)
                    : location;
        } finally {
            if (!published) {
                resolver.delete(uri, null, null);
            }
        }
    }

    @SuppressWarnings("deprecation")
    private String publishImageLegacy(
            File cachedFile,
            String displayName,
            String mimeType) throws Exception {
        File pictures = Environment.getExternalStoragePublicDirectory(
                exportDirectory(mimeType));
        File directory = new File(pictures, "CalibRaw");
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IllegalStateException("Could not create " + directory);
        }
        File destination = AndroidStorageContract.uniqueFile(directory, displayName);
        try (InputStream input = new FileInputStream(cachedFile);
             FileOutputStream output = new FileOutputStream(destination)) {
            BoundedStreams.copy(input, output, Long.MAX_VALUE, "Export is too large");
            output.getFD().sync();
        }
        MediaScannerConnection.scanFile(
                activity,
                new String[]{destination.getAbsolutePath()},
                new String[]{mimeType},
                null);
        return destination.getAbsolutePath();
    }

    boolean onRequestPermissionsResult(
            int requestCode,
            String[] permissions,
            int[] grantResults) {
        if (requestCode != WRITE_EXPORT_PERMISSION) {
            return false;
        }
        PendingLegacyExport pending = pendingLegacyExport.getAndSet(null);
        if (grantResults.length > 0
                && grantResults[0] == PackageManager.PERMISSION_GRANTED
                && pending != null) {
            startPublishThread(
                    pending.cachedPath,
                    pending.displayName,
                    pending.mimeType);
        } else {
            if (pending != null) {
                deleteCachedExport(pending.cachedPath);
            }
            callbacks.onExportPublished(
                    "",
                    "Storage permission is required to export on Android 8 and 9");
        }
        return true;
    }

    void scavengeCachedExports() {
        File directory = new File(activity.getCacheDir(), "exports");
        File[] cachedExports = directory.listFiles();
        if (cachedExports == null) {
            return;
        }
        long now = System.currentTimeMillis();
        for (File cached : cachedExports) {
            if (Thread.currentThread().isInterrupted()) {
                return;
            }
            long modified = cached.lastModified();
            boolean isStale = cached.isFile()
                    && modified > 0L
                    && now >= modified
                    && now - modified >= STALE_EXPORT_CACHE_AGE_MS;
            if (isStale) {
                deleteCachedExport(cached);
            }
        }
    }

    private static void deleteCachedExport(String cachedPath) {
        deleteCachedExport(new File(cachedPath));
    }

    private static void deleteCachedExport(File cached) {
        try {
            for (int attempt = 1; attempt <= DELETE_ATTEMPTS; attempt++) {
                if (!cached.exists() || cached.delete()) {
                    return;
                }
                if (attempt < DELETE_ATTEMPTS) {
                    Thread.yield();
                }
            }
            Log.w(
                    LOG_TAG,
                    "Could not delete export-cache file after " + DELETE_ATTEMPTS
                            + " attempts; the export-cache scavenger will retry stale files: "
                            + cached);
        } catch (RuntimeException error) {
            Log.w(
                    LOG_TAG,
                    "Could not delete export-cache file; "
                            + "the export-cache scavenger will retry stale files: " + cached,
                    error);
        }
    }

    private static final class PendingLegacyExport {
        final String cachedPath;
        final String displayName;
        final String mimeType;

        PendingLegacyExport(String cachedPath, String displayName, String mimeType) {
            this.cachedPath = cachedPath;
            this.displayName = displayName;
            this.mimeType = mimeType;
        }
    }
}
