package de.duecki.calibraw;

import android.content.ContentResolver;
import android.content.ContentUris;
import android.content.ContentValues;
import android.content.Context;
import android.database.Cursor;
import android.net.Uri;
import android.os.Build;
import android.os.Environment;
import android.os.ParcelFileDescriptor;
import android.provider.MediaStore;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.OutputStream;

/** Published logs that Files can share even when the native renderer cannot start. */
final class DiagnosticLogs {
    private static final String CURRENT = "calibraw-session.log";
    private static final String PREVIOUS = "calibraw-previous.log";
    private static final String CRASH = "calibraw-crash.log";
    private static final String RELATIVE_PATH = Environment.DIRECTORY_DOWNLOADS + "/CalibRaw/";

    private DiagnosticLogs() {}

    /** Rust takes sole ownership of the detached descriptor and closes it. */
    static String open(Context context) throws IOException {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            // On these releases app-specific external storage is visible in
            // Files, without requesting the legacy broad storage permission.
            File directory = context.getExternalFilesDir("diagnostics");
            if (directory == null) {
                throw new IOException("External diagnostic storage is unavailable");
            }
            File current = new File(directory, CURRENT);
            File previous = new File(directory, PREVIOUS);
            if (current.exists()) {
                if (previous.exists() && !previous.delete()) {
                    throw new IOException("Could not replace the previous diagnostic log");
                }
                if (!current.renameTo(previous)) {
                    throw new IOException("Could not preserve the previous diagnostic log");
                }
            }
            try (ParcelFileDescriptor descriptor = ParcelFileDescriptor.open(current,
                    ParcelFileDescriptor.MODE_CREATE | ParcelFileDescriptor.MODE_TRUNCATE
                            | ParcelFileDescriptor.MODE_READ_WRITE)) {
                return descriptor.detachFd() + "\t" + current.getAbsolutePath();
            }
        }

        ContentResolver resolver = context.getContentResolver();
        Uri current = find(context, CURRENT);
        if (current != null) {
            Uri previous = find(context, PREVIOUS);
            if (previous != null) {
                resolver.delete(previous, null, null);
            }
            ContentValues values = new ContentValues();
            values.put(MediaStore.MediaColumns.DISPLAY_NAME, PREVIOUS);
            if (resolver.update(current, values, null, null) != 1) {
                throw new IOException("Could not preserve the previous diagnostic log");
            }
        }

        // Rust has already archived the private previous session. Keep the
        // last abnormal one public too, across later successful launches.
        File crash = new File(context.getFilesDir(), "diagnostics/crash.log");
        if (crash.isFile()) {
            Uri destination = find(context, CRASH);
            if (destination == null) {
                destination = create(context, CRASH);
            }
            try (FileInputStream input = new FileInputStream(crash);
                    OutputStream output = resolver.openOutputStream(destination, "wt")) {
                if (output == null) {
                    throw new IOException("Could not open the public crash log");
                }
                byte[] buffer = new byte[8192];
                int count;
                while ((count = input.read(buffer)) != -1) {
                    output.write(buffer, 0, count);
                }
            }
        }

        Uri output = create(context, CURRENT);
        try (ParcelFileDescriptor descriptor = resolver.openFileDescriptor(output, "rwt")) {
            if (descriptor == null) {
                throw new IOException("Could not open the public diagnostic log");
            }
            return descriptor.detachFd() + "\t" + RELATIVE_PATH + CURRENT;
        }
    }

    private static Uri find(Context context, String name) {
        String selection = MediaStore.MediaColumns.DISPLAY_NAME + "=? AND "
                + MediaStore.MediaColumns.RELATIVE_PATH + "=? AND "
                + MediaStore.MediaColumns.OWNER_PACKAGE_NAME + "=?";
        try (Cursor cursor = context.getContentResolver().query(
                MediaStore.Downloads.EXTERNAL_CONTENT_URI,
                new String[] {MediaStore.MediaColumns._ID}, selection,
                new String[] {name, RELATIVE_PATH, context.getPackageName()}, null)) {
            return cursor != null && cursor.moveToFirst()
                    ? ContentUris.withAppendedId(MediaStore.Downloads.EXTERNAL_CONTENT_URI,
                            cursor.getLong(0))
                    : null;
        }
    }

    private static Uri create(Context context, String name) throws IOException {
        ContentValues values = new ContentValues();
        values.put(MediaStore.MediaColumns.DISPLAY_NAME, name);
        values.put(MediaStore.MediaColumns.MIME_TYPE, "text/plain");
        values.put(MediaStore.MediaColumns.RELATIVE_PATH, RELATIVE_PATH);
        // Publish immediately: a crash must not leave the log hidden/pending.
        values.put(MediaStore.MediaColumns.IS_PENDING, 0);
        Uri result = context.getContentResolver().insert(
                MediaStore.Downloads.EXTERNAL_CONTENT_URI, values);
        if (result == null) {
            throw new IOException("Could not create the public diagnostic log");
        }
        return result;
    }
}
