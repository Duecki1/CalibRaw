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
 * Copies picked .cube files into a staging folder in the cache. Rust imports
 * them into the LUT library, which validates them, and deletes the folder.
 */
final class LutImporter {
    // Matches MAX_CUBE_FILE_BYTES in calibraw-core.
    static final long MAX_LUT_FILE_BYTES = 32L * 1024L * 1024L;
    private static final int MAX_LUT_FILES = 256;
    private static final String STAGING_PREFIX = "lut-import-";

    interface Callbacks {
        /** An empty folder and error mean the picker was cancelled. */
        void onFilesPicked(String stagingDirectory, String error);
    }

    private final Activity storage;
    private final Callbacks callbacks;

    LutImporter(Activity storage, Callbacks callbacks) {
        this.storage = storage;
        this.callbacks = callbacks;
    }

    Intent createFilePickerIntent() {
        // .cube files have no registered MIME type, so any file can be picked.
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("*/*");
        intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
        return intent;
    }

    void handleFilePickerResult(int resultCode, Intent data) {
        List<Uri> uris = pickedUris(resultCode, data);
        if (uris.isEmpty()) {
            callbacks.onFilesPicked("", "");
            return;
        }
        new Thread(() -> stageFiles(uris), "CalibRaw LUT import").start();
    }

    private static List<Uri> pickedUris(int resultCode, Intent data) {
        List<Uri> uris = new ArrayList<>();
        if (resultCode != CalibRawActivity.RESULT_OK || data == null) {
            return uris;
        }
        ClipData clip = data.getClipData();
        if (clip != null) {
            for (int index = 0; index < clip.getItemCount() && uris.size() < MAX_LUT_FILES; index++) {
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
                storage.getCacheDir(), STAGING_PREFIX + Long.toUnsignedString(System.nanoTime()));
        try {
            if (!staging.mkdirs()) {
                throw new IllegalStateException("Could not create the LUT import folder");
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
                            MAX_LUT_FILE_BYTES,
                            "A LUT file is larger than " + MAX_LUT_FILE_BYTES + " bytes");
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
        return "LUT.cube";
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
