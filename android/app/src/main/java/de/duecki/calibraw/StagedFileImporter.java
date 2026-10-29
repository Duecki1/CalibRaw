package de.duecki.calibraw;

import android.app.Activity;
import android.content.ClipData;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.OpenableColumns;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.List;

/**
 * Copies picked files into a staging folder in the cache. Rust imports them,
 * validating each, and deletes the folder.
 */
final class StagedFileImporter {
    /** What one picker accepts and how its files are staged. */
    static final class Spec {
        final String label;
        final String mimeType;
        final int maxFiles;
        final long maxFileBytes;
        final String stagingPrefix;
        final String fallbackName;

        Spec(
                String label,
                String mimeType,
                int maxFiles,
                long maxFileBytes,
                String stagingPrefix,
                String fallbackName) {
            this.label = label;
            this.mimeType = mimeType;
            this.maxFiles = maxFiles;
            this.maxFileBytes = maxFileBytes;
            this.stagingPrefix = stagingPrefix;
            this.fallbackName = fallbackName;
        }
    }

    // .cube files have no registered MIME type, so any file can be picked.
    // The size matches MAX_CUBE_FILE_BYTES in calibraw-core.
    static final Spec LUTS = new Spec(
            "LUT", "*/*", 256, 32L * 1024L * 1024L, "lut-import-", "LUT.cube");
    // The size matches MAX_WATERMARK_FILE_BYTES in calibraw-ui.
    static final Spec WATERMARK = new Spec(
            "watermark", "image/*", 1, 64L * 1024L * 1024L, "watermark-import-", "watermark.png");

    interface Callbacks {
        /** An empty folder and error mean the picker was cancelled. */
        void onFilesPicked(String stagingDirectory, String error);
    }

    private final Activity storage;
    private final Spec spec;
    private final Callbacks callbacks;

    StagedFileImporter(Activity storage, Spec spec, Callbacks callbacks) {
        this.storage = storage;
        this.spec = spec;
        this.callbacks = callbacks;
    }

    Intent createFilePickerIntent() {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType(spec.mimeType);
        intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, spec.maxFiles > 1);
        return intent;
    }

    void handleFilePickerResult(int resultCode, Intent data) {
        List<Uri> uris = pickedUris(resultCode, data);
        if (uris.isEmpty()) {
            callbacks.onFilesPicked("", "");
            return;
        }
        new Thread(() -> stageFiles(uris), "CalibRaw " + spec.label + " import").start();
    }

    private List<Uri> pickedUris(int resultCode, Intent data) {
        List<Uri> uris = new ArrayList<>();
        if (resultCode != CalibRawActivity.RESULT_OK || data == null) {
            return uris;
        }
        ClipData clip = data.getClipData();
        if (clip != null) {
            for (int index = 0; index < clip.getItemCount() && uris.size() < spec.maxFiles; index++) {
                Uri uri = clip.getItemAt(index).getUri();
                if (uri != null) {
                    uris.add(uri);
                }
            }
        } else if (data.getData() != null) {
            uris.add(data.getData());
        }
        return uris;
    }

    private void stageFiles(List<Uri> uris) {
        File staging = new File(
                storage.getCacheDir(),
                spec.stagingPrefix + Long.toUnsignedString(System.nanoTime()));
        try {
            if (!staging.mkdirs()) {
                throw new IllegalStateException(
                        "Could not create the " + spec.label + " import folder");
            }
            for (int index = 0; index < uris.size(); index++) {
                Uri uri = uris.get(index);
                // The index keeps files with the same name apart.
                File destination = new File(staging, index + "-" + safeFileName(queryName(uri)));
                try (InputStream input = storage.getContentResolver().openInputStream(uri);
                        OutputStream output = new FileOutputStream(destination)) {
                    if (input == null) {
                        throw new IllegalStateException("Android storage could not open " + uri);
                    }
                    BoundedStreams.copy(
                            input,
                            output,
                            spec.maxFileBytes,
                            "A " + spec.label + " file is larger than "
                                    + spec.maxFileBytes + " bytes");
                }
            }
            String stagedPath = staging.getAbsolutePath();
            storage.runOnUiThread(() -> callbacks.onFilesPicked(stagedPath, ""));
        } catch (Exception error) {
            deleteStaging(staging);
            String message = error.toString();
            storage.runOnUiThread(() -> callbacks.onFilesPicked("", message));
        }
    }

    private String queryName(Uri uri) {
        try (Cursor cursor = storage.getContentResolver().query(
                uri, new String[]{OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int column = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                if (column >= 0) {
                    String name = cursor.getString(column);
                    if (name != null && !name.isEmpty()) {
                        return name;
                    }
                }
            }
        } catch (Exception ignored) {
        }
        return spec.fallbackName;
    }

    // Keeps the name readable for the library while removing path separators.
    private static String safeFileName(String name) {
        String safe = name.replaceAll("[/\\\\\\x00]", "-");
        return safe.length() > 120 ? safe.substring(safe.length() - 120) : safe;
    }

    private static void deleteStaging(File staging) {
        File[] files = staging.listFiles();
        if (files != null) {
            for (File file : files) {
                //noinspection ResultOfMethodCallIgnored
                file.delete();
            }
        }
        //noinspection ResultOfMethodCallIgnored
        staging.delete();
    }
}
