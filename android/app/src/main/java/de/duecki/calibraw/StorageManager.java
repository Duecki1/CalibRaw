package de.duecki.calibraw;

import android.app.Activity;
import android.content.ClipData;
import android.content.ContentResolver;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.provider.OpenableColumns;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.util.Log;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashSet;
import java.util.List;

final class StorageManager {
    private static final String LOG_TAG = "CalibRaw";
    static final long MAX_RAW_IMPORT_BYTES = 2_000_000_000L;
    private static final long MAX_SIDECAR_BYTES = LibraryStore.MAX_SIDECAR_BYTES;
    private static final int MAX_RAW_LIBRARY_FILES = 20_000;
    static final long STALE_TEMP_FILE_AGE_MS = 24L * 60L * 60L * 1000L;
    private static final String RAW_PICKER_URI_KEY = "raw-document-uri";

    /** Receives the outcome of copying documents into the library. */
    interface ImportCallbacks {
        void onFilePickedFd(int fd, String displayName, String libraryUri, String error);

        /** Reports a multi-photo import before each copy; always followed by a finished call. */
        void onImportProgress(int completedCount, int totalCount);

        void onImportBatchFinished(int importedCount, int failedCount, String errors);
    }

    interface Callbacks extends ImportCallbacks {
        void onFilePicked(
                String cachedPath,
                String displayName,
                String libraryUri,
                String error,
                boolean temporary);

        /** An empty key and error mean the folder picker was cancelled. */
        void onFolderLinked(String key, String error);

        /** The selected linked folder changed while the app was in the background. */
        void onLibraryChanged();
    }

    /** The store and folder that are listed and imported into; replaced as one. */
    /** The folders a listing of {@code folder} in {@code store} read, {@code folder} first. */
    private static final class Listing {
        final LibraryStore store;
        final String folder;
        final List<String> folders;

        Listing(LibraryStore store, String folder, List<String> folders) {
            this.store = store;
            this.folder = folder;
            this.folders = folders;
        }
    }

    /** A listed photo and the folder below the listed one that holds it, "" for that folder. */
    private static final class ListedDocument {
        final LibraryStore.Document document;
        final String subfolder;

        ListedDocument(LibraryStore.Document document, String subfolder) {
            this.document = document;
            this.subfolder = subfolder;
        }
    }

    private static final class Selection {
        final LibraryStore store;
        final String folder;

        Selection(LibraryStore store, String folder) {
            this.store = store;
            this.folder = folder;
        }
    }

    private final Activity storage;
    private final Callbacks callbacks;
    private final ImportCallbacks externalCallbacks;
    private final ThumbnailCache thumbnailCache;
    private final PickerLocationStore pickerLocations;
    private final FileLibraryStore fileStore;
    private final LinkedFolders linkedFolders;
    private volatile Selection selection;
    /** What the last listing read; null before the first. */
    private volatile Listing listing;

    /**
     * @param externalCallbacks receives photos other apps sent through "Open with" or Share.
     *     They are kept apart from {@code callbacks} because Rust matches those results to the
     *     open it requested.
     */
    StorageManager(Activity storage, Callbacks callbacks, ImportCallbacks externalCallbacks) {
        this.storage = storage;
        this.callbacks = callbacks;
        this.externalCallbacks = externalCallbacks;
        this.thumbnailCache = new ThumbnailCache(storage);
        this.pickerLocations = new PickerLocationStore(storage);
        this.fileStore = new FileLibraryStore(storage);
        this.linkedFolders = new LinkedFolders(storage);
        this.selection = new Selection(fileStore, "");
    }

    /** The store that listed {@code uri}. */
    private LibraryStore storeOf(Uri uri) {
        if (!TreeLibraryStore.SCHEME.equals(uri.getScheme())) {
            return fileStore;
        }
        TreeLibraryStore store = linkedFolders.store(uri.getAuthority());
        if (store == null) {
            throw new IllegalStateException("This photo's folder is no longer linked");
        }
        return store;
    }

    Intent createLinkFolderPickerIntent() {
        return linkedFolders.createPickerIntent();
    }

    void handleLinkFolderResult(int resultCode, Intent data) {
        if (resultCode != CalibRawActivity.RESULT_OK || data == null || data.getData() == null) {
            callbacks.onFolderLinked("", "");
            return;
        }
        Uri treeUri = data.getData();
        new Thread(() -> {
            try {
                callbacks.onFolderLinked(linkedFolders.link(treeUri), "");
            } catch (Exception error) {
                Log.w(LOG_TAG, "Could not link a folder", error);
                callbacks.onFolderLinked("", String.valueOf(error.getMessage()));
            }
        }, "CalibRaw link folder").start();
    }

    String listLinkedFolders() {
        return linkedFolders.encodedList();
    }

    void unlinkFolder(String key) {
        TreeLibraryStore unlinked = linkedFolders.store(key);
        linkedFolders.unlink(key);
        if (unlinked != null && selection.store == unlinked) {
            selection = new Selection(fileStore, "");
        }
    }

    /** Lists from the top of a linked folder, or of CalibRaw's library for an empty key. */
    void selectRawLibraryRoot(String key) {
        if (key.isEmpty()) {
            selection = new Selection(fileStore, "");
            return;
        }
        TreeLibraryStore store = linkedFolders.store(key);
        if (store == null) {
            throw new IllegalArgumentException("This folder is no longer linked");
        }
        selection = new Selection(store, "");
    }

    /**
     * Reports a change sync made to the selected linked folder while the app was away, or to a
     * subfolder the library shows with it.
     */
    void checkLinkedFolderForChanges() {
        Selection current = selection;
        if (!(current.store instanceof TreeLibraryStore)) {
            return;
        }
        Listing listed = listing;
        List<String> folders = listed != null
                && listed.store == current.store
                && listed.folder.equals(current.folder)
                ? listed.folders
                : Collections.singletonList(current.folder);
        new Thread(() -> {
            try {
                for (String folder : folders) {
                    if (((TreeLibraryStore) current.store).changedSinceListing(folder)) {
                        callbacks.onLibraryChanged();
                        return;
                    }
                }
            } catch (Exception error) {
                // Only a folder path the store rejects reaches here; one it cannot read is
                // reported as changed, so the listing shows the error.
                Log.w(LOG_TAG, "Could not check the linked folder for changes", error);
            }
        }, "CalibRaw linked folder check").start();
    }

    Intent createRawDocumentPickerIntent() {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("*/*");
        intent.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true);
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        Uri initialUri = pickerLocations.readContentUri(RAW_PICKER_URI_KEY);
        if (initialUri != null) {
            intent.putExtra(DocumentsContract.EXTRA_INITIAL_URI, initialUri);
        }
        return intent;
    }

    void handleRawDocumentResult(int resultCode, Intent data) {
        if (resultCode != CalibRawActivity.RESULT_OK || data == null) {
            callbacks.onFilePicked("", "", "", "", false);
            return;
        }
        ArrayList<Uri> uris = selectedDocumentUris(data);
        if (uris.isEmpty()) {
            callbacks.onFilePicked("", "", "", "", false);
            return;
        }
        new Thread(
                () -> importDocuments(uris, callbacks, true),
                uris.size() == 1 ? "CalibRaw document import" : "CalibRaw document batch import")
                .start();
    }

    /**
     * Imports photos another app sent with ACTION_VIEW, ACTION_SEND or ACTION_SEND_MULTIPLE.
     * Returns false when the intent carries no photo, for example a launcher start.
     */
    boolean openExternalDocuments(Intent intent) {
        ArrayList<Uri> uris = externalDocumentUris(intent);
        if (uris.isEmpty()) {
            return false;
        }
        // The sender's URI is not a DocumentsContract location, so it must not become the
        // picker's initial folder.
        new Thread(
                () -> importDocuments(uris, externalCallbacks, false),
                "CalibRaw shared photo import").start();
        return true;
    }

    @SuppressWarnings("deprecation") // The typed getParcelableExtra overloads need API 33.
    private static ArrayList<Uri> externalDocumentUris(Intent intent) {
        String action = intent.getAction();
        if (Intent.ACTION_VIEW.equals(action)) {
            return selectedDocumentUris(intent);
        }
        ArrayList<Uri> uris = new ArrayList<>();
        HashSet<String> seen = new HashSet<>();
        if (Intent.ACTION_SEND.equals(action)) {
            Object stream = intent.getParcelableExtra(Intent.EXTRA_STREAM);
            if (stream instanceof Uri && seen.add(stream.toString())) {
                uris.add((Uri) stream);
            }
        } else if (Intent.ACTION_SEND_MULTIPLE.equals(action)) {
            ArrayList<?> streams = intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
            if (streams != null) {
                for (Object stream : streams) {
                    if (stream instanceof Uri && seen.add(stream.toString())) {
                        uris.add((Uri) stream);
                    }
                }
            }
        }
        return uris;
    }

    void scavengeTemporaryRawFiles() {
        if (Thread.currentThread().isInterrupted()) {
            return;
        }
        long now = System.currentTimeMillis();
        // Export staging has its own scavenger, and the persistent thumbnail cache uses
        // bounded local deletion retries; neither is delegated to this RAW cleanup pass.
        File[] cachedFiles = storage.getCacheDir().listFiles((directory, name) ->
                name.startsWith("calibraw-library-")
                        || name.startsWith("calibraw-import-")
                        || name.startsWith("calibraw-sidecar-")
                        || name.startsWith("calibraw-thumbnail-"));
        deleteStaleFiles(cachedFiles, now);
        if (Thread.currentThread().isInterrupted()) {
            return;
        }

        // Copies of a linked folder's AI results, dated by their last use; the next open copies
        // what it needs again.
        deleteStaleFiles(
                new File(storage.getCacheDir(), "linked-companions").listFiles(), now);
        if (Thread.currentThread().isInterrupted()) {
            return;
        }

        fileStore.scavengeTemporaryFiles();
    }

    private static void deleteStaleFiles(File[] files, long now) {
        if (files == null) {
            return;
        }
        for (File file : files) {
            if (Thread.currentThread().isInterrupted()) {
                return;
            }
            deleteStaleFile(file, now);
        }
    }

    static void deleteStaleFile(File file, long now) {
        try {
            long modified = file.lastModified();
            boolean isStale = modified > 0L
                    && now >= modified
                    && now - modified >= STALE_TEMP_FILE_AGE_MS;
            if (file.isFile() && isStale && !file.delete() && file.exists()) {
                Log.w(
                        LOG_TAG,
                        "Could not delete stale RAW temporary file; "
                                + "a later RAW scavenging pass will retry: " + file);
            }
        } catch (RuntimeException error) {
            Log.w(LOG_TAG, "Could not clean up stale RAW temporary file " + file, error);
        }
    }

    private static ArrayList<Uri> selectedDocumentUris(Intent data) {
        ArrayList<Uri> uris = new ArrayList<>();
        HashSet<String> seen = new HashSet<>();
        ClipData clipData = data.getClipData();
        if (clipData != null) {
            for (int index = 0; index < clipData.getItemCount(); index++) {
                Uri uri = clipData.getItemAt(index).getUri();
                if (uri != null && seen.add(uri.toString())) {
                    uris.add(uri);
                }
            }
        }
        Uri single = data.getData();
        if (single != null && seen.add(single.toString())) {
            uris.add(single);
        }
        return uris;
    }

    private void importDocuments(
            ArrayList<Uri> uris, ImportCallbacks target, boolean rememberPickerLocation) {
        if (uris.size() == 1) {
            Uri uri = uris.get(0);
            importSingleDocument(uri, queryDisplayName(uri), target, rememberPickerLocation);
            return;
        }

        int imported = 0;
        int failed = 0;
        ArrayList<String> errors = new ArrayList<>();
        for (Uri uri : uris) {
            target.onImportProgress(imported + failed, uris.size());
            String displayName = queryDisplayName(uri);
            LibraryStore.Stored stored = null;
            try {
                stored = importDocumentIntoLibrary(uri, displayName, target);
                if (imported == 0 && rememberPickerLocation) {
                    pickerLocations.writeContentUri(RAW_PICKER_URI_KEY, uri);
                }
                imported++;
            } catch (Exception error) {
                if (stored != null) {
                    deleteStoredRaw(stored.uri, stored.displayName);
                }
                failed++;
                if (errors.size() < 4) {
                    errors.add(displayName + ": " + error);
                }
            }
        }
        if (failed > errors.size()) {
            errors.add((failed - errors.size()) + " additional import(s) failed");
        }
        target.onImportBatchFinished(imported, failed, String.join("\n", errors));
    }

    private void importSingleDocument(
            Uri uri,
            String displayName,
            ImportCallbacks target,
            boolean rememberPickerLocation) {
        LibraryStore.Stored stored = null;
        try {
            stored = importDocumentIntoLibrary(uri, displayName, target);
            if (rememberPickerLocation) {
                pickerLocations.writeContentUri(RAW_PICKER_URI_KEY, uri);
            }
            deliverLibraryRawFd(stored.uri, stored.displayName, target);
        } catch (Exception error) {
            if (stored != null) {
                deleteStoredRaw(stored.uri, stored.displayName);
            }
            target.onFilePickedFd(-1, displayName, "", error.toString());
        }
    }

    private LibraryStore.Stored importDocumentIntoLibrary(
            Uri uri, String displayName, ImportCallbacks target) throws Exception {
        if (!AndroidStorageContract.isSupportedImageName(displayName)) {
            throw new IllegalArgumentException(
                    "Choose a supported photo (for example DNG, CR3, NEF, ARW, JPEG, PNG, or HEIC)");
        }
        Long declaredSize = queryDocumentSize(uri);
        if (declaredSize != null && declaredSize > MAX_RAW_IMPORT_BYTES) {
            throw new IllegalStateException(
                    "The selected RAW is " + declaredSize
                            + " bytes; the Android import limit is "
                            + MAX_RAW_IMPORT_BYTES);
        }
        return storeRawFile(uri, displayName, importSelection(target));
    }

    /**
     * Photos another app sent are not written into a folder the user syncs: they go to CalibRaw's
     * library. Imports the user asked for go to the folder on screen.
     */
    private Selection importSelection(ImportCallbacks target) {
        Selection current = selection;
        return target == externalCallbacks && current.store != fileStore
                ? new Selection(fileStore, "")
                : current;
    }

    String rawLibraryLocation() {
        return selection.store.location();
    }

    /**
     * The photos of the selected folder and, with {@code includeSubfolders}, of every folder
     * below it.
     */
    String listRawLibrary(boolean includeSubfolders) {
        try {
            return encodeRawLibrary(includeSubfolders);
        } catch (Exception error) {
            throw new IllegalStateException("Could not list the RAW library", error);
        }
    }

    String listRawLibraryFolders() {
        try {
            return selection.store.listFolders();
        } catch (Exception error) {
            throw new IllegalStateException("Could not list RAW library folders", error);
        }
    }

    /**
     * The folders of the linked folder {@code key}, or of CalibRaw's library for an empty key, as
     * {@link #listRawLibraryFolders} lists them. Named rather than taken from the selection, as
     * the sidebar shows every root's folders and walking a large tree outlasts selecting another
     * folder.
     */
    String listFolderTree(String key) {
        LibraryStore store = key.isEmpty() ? fileStore : linkedFolders.store(key);
        if (store == null) {
            throw new IllegalArgumentException("This folder is no longer linked");
        }
        try {
            return store.listFolders();
        } catch (Exception error) {
            throw new IllegalStateException("Could not list the folder's folders", error);
        }
    }

    void selectRawLibraryFolder(String relativePath) throws Exception {
        LibraryStore store = selection.store;
        selection = new Selection(store, store.normalizeFolder(relativePath));
    }

    String createRawLibraryFolder(String parentPath, String requestedName) throws Exception {
        return selection.store.createFolder(parentPath, requestedName);
    }

    int openRawLibraryFd(String uriText) throws Exception {
        Uri uri = Uri.parse(uriText);
        ParcelFileDescriptor descriptor = storeOf(uri).openDescriptor(uri);
        return NativeFileDescriptors.detach(
                descriptor, "The RAW library returned no file descriptor");
    }

    /** Where library previews are cached; native code names and reads the files in it. */
    String thumbnailCacheDirectory() {
        return thumbnailCache.directory().getAbsolutePath();
    }

    void clearThumbnailCache() {
        thumbnailCache.clear();
    }

    long thumbnailCacheSizeBytes() {
        return thumbnailCache.sizeBytes();
    }

    void maintainThumbnailCache() {
        thumbnailCache.maintain();
    }

    String materializeRawLibraryDocument(String uriText, String displayName) throws Exception {
        Uri uri = Uri.parse(uriText);
        storeOf(uri).verify(uri, displayName);
        String safeName = AndroidStorageContract.safeRawName(displayName);
        int dot = safeName.lastIndexOf('.');
        String suffix = dot >= 0 ? safeName.substring(dot) : ".raw";
        File cached = File.createTempFile("calibraw-library-", suffix, storage.getCacheDir());
        boolean completed = false;
        try {
            try (InputStream input = storeOf(uri).openInput(uri);
                 FileOutputStream output = new FileOutputStream(cached)) {
                if (input == null) {
                    throw new IllegalStateException("Android storage returned no RAW stream");
                }
                BoundedStreams.copy(
                        input,
                        output,
                        MAX_RAW_IMPORT_BYTES,
                        storageLimitMessage(MAX_RAW_IMPORT_BYTES));
                output.getFD().sync();
            }
            completed = true;
            return cached.getAbsolutePath();
        } finally {
            if (!completed) {
                deleteRawTemporaryFile(cached, "incomplete RAW materialization");
            }
        }
    }

    String materializeRawLibraryThumbnail(String uriText, String displayName) throws Exception {
        return materializeRawLibraryDocument(uriText, displayName);
    }

    void openRawLibraryDocument(String uriText, String displayName) {
        new Thread(
                () -> {
                    try {
                        deliverLibraryRawFd(Uri.parse(uriText), displayName, callbacks);
                    } catch (Exception error) {
                        callbacks.onFilePickedFd(-1, displayName, uriText, error.toString());
                    }
                },
                "CalibRaw library open").start();
    }

    void removeRawSidecar(String rawUriText, String displayName) throws Exception {
        Uri rawUri = Uri.parse(rawUriText);
        LibraryStore store = storeOf(rawUri);
        store.verify(rawUri, displayName);
        store.deleteEdits(rawUri, displayName);
    }

    /**
     * Returns the local file for the RAW's AI-denoise result, which native code reads and writes
     * directly: it is too large to stage through the cache directory like the sidecar.
     */
    String aiDenoiseResultPath(String rawUriText, String displayName) throws Exception {
        return companionPath(rawUriText, displayName, AndroidStorageContract.AI_DENOISE_SUFFIX);
    }

    /**
     * Returns the local file for the RAW's people analysis, which native code reads and writes
     * directly, as it does the AI-denoise result.
     */
    String peopleAnalysisPath(String rawUriText, String displayName) throws Exception {
        return companionPath(
                rawUriText, displayName, AndroidStorageContract.PEOPLE_ANALYSIS_SUFFIX);
    }

    private String companionPath(String rawUriText, String displayName, String suffix)
            throws Exception {
        Uri rawUri = Uri.parse(rawUriText);
        LibraryStore store = storeOf(rawUri);
        store.verify(rawUri, displayName);
        return store.companionPath(rawUri, displayName, suffix);
    }

    void fetchPhotoCompanion(String rawUriText, String displayName, String suffix)
            throws Exception {
        Uri rawUri = Uri.parse(rawUriText);
        storeOf(rawUri).fetchCompanion(rawUri, displayName, suffix);
    }

    void publishPhotoCompanion(String rawUriText, String displayName, String suffix)
            throws Exception {
        Uri rawUri = Uri.parse(rawUriText);
        storeOf(rawUri).publishCompanion(rawUri, displayName, suffix);
    }

    void removePhotoCompanion(String rawUriText, String displayName, String suffix)
            throws Exception {
        Uri rawUri = Uri.parse(rawUriText);
        LibraryStore store = storeOf(rawUri);
        store.verify(rawUri, displayName);
        store.removeCompanion(rawUri, displayName, suffix);
    }

    boolean hasPhotoCompanion(String rawUriText, String displayName, String suffix)
            throws Exception {
        Uri rawUri = Uri.parse(rawUriText);
        LibraryStore store = storeOf(rawUri);
        store.verify(rawUri, displayName);
        return store.hasCompanion(rawUri, displayName, suffix);
    }

    String importLocalRawLibraryDocument(String rawPath, String displayName) throws Exception {
        File sourceRaw = new File(rawPath);
        if (!sourceRaw.isFile() || !AndroidStorageContract.isSupportedImageName(displayName)) {
            throw new IllegalArgumentException("The local photo is missing or unsupported");
        }
        LibraryStore.Stored imported = null;
        try {
            imported = storeRawFile(Uri.fromFile(sourceRaw), displayName, selection);
            File sourceSidecar = new File(rawPath + ".calibraw");
            if (sourceSidecar.isFile()) {
                publishRawSidecar(
                        sourceSidecar.getAbsolutePath(),
                        imported.uri.toString(),
                        imported.displayName);
            }
            return imported.uri.toString() + "\n" + imported.displayName;
        } catch (Exception error) {
            if (imported != null) {
                try {
                    deleteRawLibraryDocument(imported.uri.toString(), imported.displayName);
                } catch (Exception ignored) {
                }
            }
            throw error;
        }
    }

    void deleteImportedRawLibraryDocument(String rawUri, String displayName) throws Exception {
        deleteRawLibraryDocument(rawUri, displayName);
    }

    String renameRawLibraryDocument(
            String rawUriText,
            String displayName,
            String requestedName) throws Exception {
        Uri rawUri = Uri.parse(rawUriText);
        LibraryStore store = storeOf(rawUri);
        store.verify(rawUri, displayName);
        String safeName = AndroidStorageContract.safeRawName(requestedName);
        if (!safeName.equals(requestedName) || !AndroidStorageContract.isSupportedImageName(requestedName)) {
            throw new IllegalArgumentException("Enter a safe supported photo filename");
        }
        if (displayName.equals(requestedName)) {
            return rawUri.toString();
        }
        // Native code moves the developed thumbnail cached under the old URI.
        return store.rename(rawUri, displayName, requestedName).toString();
    }

    void deleteRawLibraryDocument(String rawUriText, String displayName) throws Exception {
        Uri rawUri = Uri.parse(rawUriText);
        LibraryStore store = storeOf(rawUri);
        store.verify(rawUri, displayName);
        // Native code clears the developed thumbnail cached under this URI.
        store.delete(rawUri, displayName);
    }

    String materializeRawSidecar(String rawUriText, String displayName) throws Exception {
        Uri rawUri = Uri.parse(rawUriText);
        LibraryStore store = storeOf(rawUri);
        store.verify(rawUri, displayName);
        try (InputStream sidecar = store.openSidecar(rawUri, displayName)) {
            return sidecar == null ? "" : copySidecarToCache(sidecar);
        }
    }

    private String copySidecarToCache(InputStream input) throws Exception {
        File cached = File.createTempFile("calibraw-sidecar-", ".calibraw", storage.getCacheDir());
        boolean completed = false;
        try {
            try (FileOutputStream output = new FileOutputStream(cached)) {
                BoundedStreams.copy(
                        input,
                        output,
                        MAX_SIDECAR_BYTES,
                        storageLimitMessage(MAX_SIDECAR_BYTES));
                output.getFD().sync();
            }
            completed = true;
            return cached.getAbsolutePath();
        } finally {
            if (!completed) {
                deleteRawTemporaryFile(cached, "incomplete sidecar materialization");
            }
        }
    }

    String createRawSidecarCache() throws Exception {
        return File.createTempFile("calibraw-sidecar-", ".calibraw", storage.getCacheDir()).getAbsolutePath();
    }

    String publishRawSidecar(
            String cachedPath,
            String rawUriText,
            String displayName) throws Exception {
        File cached = new File(cachedPath);
        if (!cached.isFile() || cached.length() > MAX_SIDECAR_BYTES) {
            throw new IllegalStateException("CalibRaw sidecar staging file is missing or too large");
        }
        Uri rawUri = Uri.parse(rawUriText);
        LibraryStore store = storeOf(rawUri);
        store.verify(rawUri, displayName);
        return store.publishSidecar(cached, rawUri, displayName);
    }

    private LibraryStore.Stored storeRawFile(Uri source, String requestedName, Selection into)
            throws Exception {
        try (InputStream input = openSourceInput(source)) {
            if (input == null) {
                throw new IllegalStateException("The document provider returned no input stream");
            }
            return into.store.store(input, requestedName, into.folder);
        }
    }

    /** A document being imported: another app's URI, or a local file. */
    private InputStream openSourceInput(Uri uri) throws Exception {
        if (ContentResolver.SCHEME_FILE.equals(uri.getScheme())) {
            return new FileInputStream(new File(uri.getPath()));
        }
        return storage.getContentResolver().openInputStream(uri);
    }

    static void deleteRawTemporaryFile(File file, String description) {
        try {
            if (!file.delete() && file.exists()) {
                Log.w(
                        LOG_TAG,
                        "Could not delete " + description
                                + "; the RAW temporary-file scavenger will retry: " + file);
            }
        } catch (RuntimeException error) {
            Log.w(
                    LOG_TAG,
                    "Could not delete " + description
                            + "; the RAW temporary-file scavenger will retry: " + file,
                    error);
        }
    }

    private void deliverLibraryRawFd(Uri source, String displayName, ImportCallbacks target)
            throws Exception {
        storeOf(source).verify(source, displayName);
        int fd = openRawLibraryFd(source.toString());
        if (!decodersCanRead(fd) && target instanceof Callbacks) {
            NativeFileDescriptors.closeTransferred(fd);
            String cached = materializeRawLibraryDocument(source.toString(), displayName);
            ((Callbacks) target).onFilePicked(cached, displayName, source.toString(), "", true);
            return;
        }
        boolean handedOff = false;
        try {
            target.onFilePickedFd(fd, displayName, source.toString(), "");
            handedOff = true;
        } finally {
            if (!handedOff) {
                NativeFileDescriptors.closeTransferred(fd);
            }
        }
    }

    /**
     * The decoders open a document as /proc/self/fd/N and seek in it. A provider may stream through
     * a pipe, which cannot seek. Reopening a file another app owns is refused even though this
     * descriptor can read it, because shared storage checks permission again on open.
     */
    private static boolean decodersCanRead(int fd) {
        try (ParcelFileDescriptor borrowed = ParcelFileDescriptor.fromFd(fd)) {
            Os.lseek(borrowed.getFileDescriptor(), 0L, OsConstants.SEEK_CUR);
            Os.close(Os.open("/proc/self/fd/" + fd, OsConstants.O_RDONLY, 0));
            return true;
        } catch (ErrnoException | java.io.IOException unreadable) {
            return false;
        }
    }

    private String encodeRawLibrary(boolean includeSubfolders) throws Exception {
        Selection current = selection;
        ArrayList<String> folders = new ArrayList<>();
        folders.add(current.folder);
        if (includeSubfolders) {
            folders.addAll(foldersBelow(current.store, current.folder));
        }
        String prefix = current.folder.isEmpty() ? "" : current.folder + "/";
        // Preserve one sentinel beyond the UI limit.
        int retained = MAX_RAW_LIBRARY_FILES + 1;
        ArrayList<ListedDocument> records = new ArrayList<>();
        ArrayList<String> listedFolders = new ArrayList<>();
        for (String folder : folders) {
            boolean top = folder.equals(current.folder);
            ArrayList<LibraryStore.Document> documents;
            try {
                documents = current.store.listDocuments(folder, retained);
            } catch (Exception error) {
                if (top) {
                    throw error;
                }
                // One unreadable subfolder leaves the others shown.
                Log.w(LOG_TAG, "Could not list a library subfolder", error);
                continue;
            }
            listedFolders.add(folder);
            String subfolder = top ? "" : folder.substring(prefix.length());
            for (LibraryStore.Document document : documents) {
                records.add(new ListedDocument(document, subfolder));
            }
            // Only the newest can be shown, which bounds the memory of many folders.
            if (records.size() > 2 * retained) {
                keepNewest(records, retained);
            }
        }
        keepNewest(records, retained);
        listing = new Listing(current.store, current.folder, listedFolders);

        StringBuilder result = new StringBuilder();
        for (ListedDocument record : records) {
            appendLibraryRecord(result, record.document, record.subfolder);
        }
        return result.toString();
    }

    /** Sorts {@code records} in output order and drops all but the first {@code limit}. */
    private static void keepNewest(ArrayList<ListedDocument> records, int limit) {
        records.sort((left, right) ->
                RAW_LIBRARY_OUTPUT_ORDER.compare(left.document, right.document));
        if (records.size() > limit) {
            records.subList(limit, records.size()).clear();
        }
    }

    /** Every folder below {@code folder} in {@code store}, as {@code listFolders} names it. */
    private static ArrayList<String> foldersBelow(LibraryStore store, String folder)
            throws Exception {
        String prefix = folder.isEmpty() ? "" : folder + "/";
        ArrayList<String> result = new ArrayList<>();
        for (String line : store.listFolders().split("\n")) {
            int tab = line.indexOf('\t');
            if (tab <= 0) {
                continue;
            }
            String path = Uri.decode(line.substring(0, tab));
            if (path.startsWith(prefix)) {
                result.add(path);
            }
        }
        return result;
    }

    /**
     * One tab-separated line: the encoded URI, name and path, the byte size, the modification
     * time in seconds, the sidecar's byte size and modification time in milliseconds, both "-"
     * when the photo has none, then the encoded subfolder, empty for the listed folder itself.
     * Read by {@code parse_library_record} in calibraw-ffi.
     */
    private static void appendLibraryRecord(
            StringBuilder result, LibraryStore.Document record, String subfolder) {
        result.append(Uri.encode(record.uri)).append('\t')
                .append(Uri.encode(record.displayName)).append('\t')
                .append(Uri.encode(record.displayPath)).append('\t')
                .append(record.bytes).append('\t')
                .append(record.modifiedSeconds).append('\t');
        if (record.sidecarBytes == LibraryStore.NO_SIDECAR) {
            result.append("-\t-");
        } else {
            result.append(record.sidecarBytes).append('\t').append(record.sidecarModifiedMillis);
        }
        result.append('\t').append(Uri.encode(subfolder)).append('\n');
    }

    private void deleteStoredRaw(Uri uri, String displayName) {
        try {
            storeOf(uri).delete(uri, displayName);
        } catch (Exception ignored) {
        }
    }

    private static final Comparator<LibraryStore.Document> RAW_LIBRARY_OUTPUT_ORDER =
            (left, right) -> {
                int modifiedOrder = Long.compare(right.modifiedSeconds, left.modifiedSeconds);
                return modifiedOrder != 0 ? modifiedOrder : left.uri.compareTo(right.uri);
            };

    static String storageLimitMessage(long maximumBytes) {
        return "The document exceeds the " + maximumBytes + "-byte import limit";
    }

    private Long queryDocumentSize(Uri uri) {
        try (Cursor cursor = storage.getContentResolver().query(
                uri,
                new String[]{OpenableColumns.SIZE},
                null,
                null,
                null)) {
            if (cursor != null && cursor.moveToFirst()) {
                int column = cursor.getColumnIndex(OpenableColumns.SIZE);
                if (column >= 0 && !cursor.isNull(column)) {
                    long size = cursor.getLong(column);
                    if (size >= 0) {
                        return size;
                    }
                }
            }
        } catch (Exception ignored) {
            // Streaming bounds remain authoritative when provider metadata is unreliable.
        }
        return null;
    }

    private String queryDisplayName(Uri uri) {
        try (Cursor cursor = storage.getContentResolver().query(
                uri,
                new String[]{OpenableColumns.DISPLAY_NAME},
                null,
                null,
                null)) {
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
            // The URI may remain readable when its optional display-name query fails.
        }
        return "selected RAW";
    }

}
