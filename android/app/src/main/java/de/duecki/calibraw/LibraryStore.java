package de.duecki.calibraw;

import android.net.Uri;
import android.os.ParcelFileDescriptor;

import java.io.File;
import java.io.InputStream;
import java.util.ArrayList;

/**
 * Where library photos and the files kept beside them live: CalibRaw's own folder, or a folder
 * the user linked. Every photo is named by a URI this store listed and its display name. Folders
 * are "/"-separated paths relative to the store's top, which is "".
 *
 * Methods reach storage, so they are called off the Android UI thread, except where noted.
 */
interface LibraryStore {
    // Matches calibraw_core::sidecar::MAX_SIDECAR_BYTES on Android; change both together.
    long MAX_SIDECAR_BYTES = 128L * 1024L * 1024L;
    int MAX_FOLDERS = 10_000;
    int MAX_FOLDER_DEPTH = 64;

    final class Document {
        final String uri;
        final String displayName;
        final String displayPath;
        final long bytes;
        final long modifiedSeconds;
        /** The sidecar's size, or {@link #NO_SIDECAR} when the photo has none. */
        final long sidecarBytes;
        final long sidecarModifiedMillis;

        Document(
                String uri,
                String displayName,
                String displayPath,
                long bytes,
                long modifiedSeconds,
                long sidecarBytes,
                long sidecarModifiedMillis) {
            this.uri = uri;
            this.displayName = displayName;
            this.displayPath = displayPath;
            this.bytes = bytes;
            this.modifiedSeconds = modifiedSeconds;
            this.sidecarBytes = sidecarBytes;
            this.sidecarModifiedMillis = sidecarModifiedMillis;
        }
    }

    long NO_SIDECAR = -1L;

    final class Stored {
        final Uri uri;
        final String displayName;

        Stored(Uri uri, String displayName) {
            this.uri = uri;
            this.displayName = displayName;
        }
    }

    /** The top folder, for people to read. Reaches no storage. */
    String location();

    /** All folders below the top as "{encoded path}\t{encoded name}\n" lines, parents first. */
    String listFolders() throws Exception;

    /**
     * The newest {@code limit} photos directly in {@code folder}, in no particular order, with
     * the size and modification time of each one's sidecar, so the library can tell which changed
     * without reading them.
     */
    ArrayList<Document> listDocuments(String folder, int limit) throws Exception;

    /** The canonical form of {@code folder}; rejects a path that leaves the store. */
    String normalizeFolder(String folder) throws Exception;

    /** Creates {@code name} under {@code parent} and returns the new folder's path. */
    String createFolder(String parent, String name) throws Exception;

    /** Rejects a URI that is not this store's photo called {@code displayName}. */
    void verify(Uri raw, String displayName) throws Exception;

    ParcelFileDescriptor openDescriptor(Uri raw) throws Exception;

    InputStream openInput(Uri raw) throws Exception;

    /** Copies {@code input} to a new photo in {@code folder}, numbering a name already taken. */
    Stored store(InputStream input, String requestedName, String folder) throws Exception;

    /** Renames a verified photo and what is kept beside it to a validated, different name. */
    Uri rename(Uri raw, String displayName, String requestedName) throws Exception;

    /** Deletes a verified photo and what is kept beside it. */
    void delete(Uri raw, String displayName) throws Exception;

    /** Deletes the sidecar and the files kept beside the photo, leaving the photo. */
    void deleteEdits(Uri raw, String displayName) throws Exception;

    /** The photo's sidecar, or null when it has none. */
    InputStream openSidecar(Uri raw, String displayName) throws Exception;

    /** Replaces the photo's sidecar with {@code cached} and returns where it was saved. */
    String publishSidecar(File cached, Uri raw, String displayName) throws Exception;

    /** The local file native code reads and writes for this companion. Copies nothing. */
    String companionPath(Uri raw, String displayName, String suffix) throws Exception;

    /** Makes {@link #companionPath} hold the stored companion, or nothing when there is none. */
    void fetchCompanion(Uri raw, String displayName, String suffix) throws Exception;

    /** Stores what native code wrote at {@link #companionPath}. */
    void publishCompanion(Uri raw, String displayName, String suffix) throws Exception;

    void removeCompanion(Uri raw, String displayName, String suffix) throws Exception;

    boolean hasCompanion(Uri raw, String displayName, String suffix) throws Exception;
}
