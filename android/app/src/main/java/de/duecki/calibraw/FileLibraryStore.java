package de.duecki.calibraw;

import android.app.Activity;
import android.content.ContentResolver;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.util.Log;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Comparator;
import java.util.PriorityQueue;

/** CalibRaw's own library in the app's shared media folder, reached by file path. */
final class FileLibraryStore implements LibraryStore {
    private static final String LOG_TAG = "CalibRaw";
    private static final long MAX_RAW_IMPORT_BYTES = StorageManager.MAX_RAW_IMPORT_BYTES;
    private static final long STALE_TEMP_FILE_AGE_MS = StorageManager.STALE_TEMP_FILE_AGE_MS;

    private final Activity storage;

    FileLibraryStore(Activity storage) {
        this.storage = storage;
    }

    void scavengeTemporaryFiles() {
        long now = System.currentTimeMillis();
        File library = rawLibraryDirectory();
        try {
            deleteStaleLibraryTemporaryFiles(library, library.getCanonicalFile(), now, 0);
        } catch (Exception error) {
            Log.w(LOG_TAG, "Could not scavenge stale RAW library temporary files", error);
        }
    }

    private static void deleteStaleLibraryTemporaryFiles(
            File directory,
            File canonicalLibrary,
            long now,
            int depth) throws Exception {
        if (Thread.currentThread().isInterrupted()
                || depth > MAX_FOLDER_DEPTH
                || !directory.isDirectory()) {
            return;
        }
        File canonicalDirectory = directory.getCanonicalFile();
        if (!canonicalDirectory.toPath().startsWith(canonicalLibrary.toPath())) {
            return;
        }
        File[] entries = directory.listFiles();
        if (entries == null) {
            return;
        }
        for (File entry : entries) {
            if (Thread.currentThread().isInterrupted()) {
                return;
            }
            if (entry.isDirectory()) {
                deleteStaleLibraryTemporaryFiles(entry, canonicalLibrary, now, depth + 1);
            } else if (AndroidStorageContract.isLibraryTemporaryFileName(entry.getName())) {
                StorageManager.deleteStaleFile(entry, now);
            }
        }
    }

    @Override
    public String location() {
        return rawLibraryDirectory().getAbsolutePath();
    }

    @Override
    public String listFolders() throws Exception {
        File root = rawLibraryDirectory();
        StringBuilder result = new StringBuilder();
        appendRawLibraryFolders(root, root, result, 0, new int[]{0});
        return result.toString();
    }

    private void appendRawLibraryFolders(
            File root,
            File directory,
            StringBuilder result,
            int depth,
            int[] count) throws Exception {
        if (depth >= MAX_FOLDER_DEPTH || count[0] >= MAX_FOLDERS) {
            return;
        }
        File[] children = directory.listFiles(file ->
                file.isDirectory() && !file.getName().startsWith("."));
        if (children == null) {
            return;
        }
        Arrays.sort(children, (left, right) ->
                left.getName().compareToIgnoreCase(right.getName()));
        for (File child : children) {
            if (count[0] >= MAX_FOLDERS) {
                return;
            }
            String relative = AndroidStorageContract.relativeLibraryFolder(root, child);
            result.append(Uri.encode(relative)).append('\t')
                    .append(Uri.encode(child.getName())).append('\n');
            count[0]++;
            appendRawLibraryFolders(root, child, result, depth + 1, count);
        }
    }

    @Override
    public ArrayList<Document> listDocuments(String folder, int limit) throws Exception {
        ArrayList<Document> result = new ArrayList<>();
        File[] files = directoryOf(folder).listFiles();
        if (files == null) {
            return result;
        }
        PriorityQueue<RawLibraryCandidate> retained = selectRawLibraryCandidates(files, limit);
        for (RawLibraryCandidate candidate : retained) {
            File file = candidate.file;
            File sidecar = sidecarOf(file);
            result.add(new Document(
                    Uri.fromFile(file).toString(),
                    file.getName(),
                    file.getAbsolutePath(),
                    Math.max(0, file.length()),
                    Math.max(0, candidate.modifiedMillis / 1000),
                    sidecar != null ? sidecar.length() : NO_SIDECAR,
                    sidecar != null ? sidecar.lastModified() : NO_SIDECAR));
        }
        return result;
    }

    /** The sidecar beside {@code photo}, or null when it has none. */
    private static File sidecarOf(File photo) {
        try {
            File sidecar = new File(
                    photo.getParentFile(),
                    AndroidStorageContract.sidecarDisplayName(photo.getName()));
            return sidecar.isFile() ? sidecar : null;
        } catch (IllegalArgumentException noSidecarName) {
            return null;
        }
    }

    private static PriorityQueue<RawLibraryCandidate> selectRawLibraryCandidates(
            File[] files, int retainedLimit) {
        PriorityQueue<RawLibraryCandidate> retained = new PriorityQueue<>(
                retainedLimit, RAW_LIBRARY_CUTOFF_WORST_FIRST);
        for (int index = 0; index < files.length; index++) {
            File file = files[index];
            long modifiedMillis = file.lastModified();
            if (!file.isFile() || !AndroidStorageContract.isSupportedImageName(file.getName())) {
                continue;
            }
            retainRawLibraryCandidate(
                    retained,
                    new RawLibraryCandidate(file, modifiedMillis, index),
                    retainedLimit);
        }
        return retained;
    }

    private static void retainRawLibraryCandidate(
            PriorityQueue<RawLibraryCandidate> retained,
            RawLibraryCandidate candidate,
            int retainedLimit) {
        if (retained.size() < retainedLimit) {
            retained.add(candidate);
            return;
        }
        RawLibraryCandidate cutoff = retained.peek();
        if (cutoff != null && RAW_LIBRARY_CUTOFF_WORST_FIRST.compare(candidate, cutoff) > 0) {
            retained.poll();
            retained.add(candidate);
        }
    }

    @Override
    public String normalizeFolder(String folder) throws Exception {
        File resolved = AndroidStorageContract.libraryFolder(rawLibraryDirectory(), folder);
        if (!resolved.isDirectory()) {
            throw new IllegalArgumentException("The selected RAW library folder does not exist");
        }
        return AndroidStorageContract.relativeLibraryFolder(rawLibraryDirectory(), resolved);
    }

    @Override
    public String createFolder(String parentPath, String requestedName) throws Exception {
        File root = rawLibraryDirectory();
        File parent = AndroidStorageContract.libraryFolder(root, parentPath);
        if (!parent.isDirectory()) {
            throw new IllegalArgumentException("The parent RAW library folder does not exist");
        }
        String name = AndroidStorageContract.safeFolderName(requestedName);
        File folder = new File(parent, name);
        if (folder.exists()) {
            throw new IllegalStateException(name + " already exists");
        }
        if (!folder.mkdir()) {
            throw new IllegalStateException("Android storage could not create folder " + name);
        }
        return AndroidStorageContract.relativeLibraryFolder(root, folder);
    }

    @Override
    public void verify(Uri rawUri, String expectedDisplayName) throws Exception {
        if (rawUri.getPath() == null || expectedDisplayName == null) {
            throw new IllegalArgumentException("The RAW library URI is invalid");
        }
        File raw = new File(rawUri.getPath());
        if (!AndroidStorageContract.isAllowedRawFile(
                raw, expectedDisplayName, rawLibraryDirectory())) {
            throw new IllegalArgumentException("The RAW is outside CalibRaw's library");
        }
    }

    @Override
    public ParcelFileDescriptor openDescriptor(Uri uri) throws Exception {
        if (ContentResolver.SCHEME_FILE.equals(uri.getScheme())) {
            return ParcelFileDescriptor.open(
                    new File(uri.getPath()), ParcelFileDescriptor.MODE_READ_ONLY);
        }
        return storage.getContentResolver().openFileDescriptor(uri, "r");
    }

    @Override
    public InputStream openInput(Uri uri) throws Exception {
        if (ContentResolver.SCHEME_FILE.equals(uri.getScheme())) {
            return new FileInputStream(new File(uri.getPath()));
        }
        return storage.getContentResolver().openInputStream(uri);
    }

    @Override
    public Stored store(InputStream input, String requestedName, String folder) throws Exception {
        File directory = directoryOf(folder);
        File destination = AndroidStorageContract.uniqueFile(
                directory, AndroidStorageContract.safeRawName(requestedName));
        File partial = AndroidStorageContract.uniqueFile(
                directory,
                AndroidStorageContract.importPartialName(destination.getName()));
        boolean completed = false;
        try {
            try (FileOutputStream output = new FileOutputStream(partial)) {
                BoundedStreams.copy(
                        input,
                        output,
                        MAX_RAW_IMPORT_BYTES,
                        StorageManager.storageLimitMessage(MAX_RAW_IMPORT_BYTES));
                output.getFD().sync();
            }
            if (!partial.renameTo(destination)) {
                throw new IllegalStateException(
                        "Could not publish the imported RAW in " + directory);
            }
            completed = true;
            return new Stored(Uri.fromFile(destination), destination.getName());
        } finally {
            if (!completed) {
                StorageManager.deleteRawTemporaryFile(partial, "incomplete RAW import");
            }
        }
    }

    @Override
    public Uri rename(Uri rawUri, String displayName, String requestedName) {
        File sourceRaw = new File(rawUri.getPath());
        File parent = sourceRaw.getParentFile();
        File destinationRaw = new File(parent, requestedName);
        File sourceSidecar =
                new File(parent, AndroidStorageContract.sidecarDisplayName(displayName));
        File destinationSidecar =
                new File(parent, AndroidStorageContract.sidecarDisplayName(requestedName));
        if (destinationRaw.exists() || destinationSidecar.exists()) {
            throw new IllegalStateException(requestedName + " already exists");
        }
        if (!sourceRaw.renameTo(destinationRaw)) {
            throw new IllegalStateException("Android storage could not rename the RAW file");
        }
        if (sourceSidecar.isFile() && !sourceSidecar.renameTo(destinationSidecar)) {
            if (!destinationRaw.renameTo(sourceRaw)) {
                throw new IllegalStateException(
                        "The RAW was renamed, but its sidecar and RAW rollback both failed");
            }
            throw new IllegalStateException("Android storage could not rename the RAW sidecar");
        }
        keepRegeneratedCompanion(
                parent,
                AndroidStorageContract.aiDenoiseDisplayName(displayName),
                AndroidStorageContract.aiDenoiseDisplayName(requestedName),
                "AI-denoise result");
        keepRegeneratedCompanion(
                parent,
                AndroidStorageContract.peopleAnalysisDisplayName(displayName),
                AndroidStorageContract.peopleAnalysisDisplayName(requestedName),
                "people analysis");
        return Uri.fromFile(destinationRaw);
    }

    /** The app can produce this file again, so a failed move only costs running that again. */
    private static void keepRegeneratedCompanion(
            File parent, String sourceName, String destinationName, String description) {
        File source = new File(parent, sourceName);
        if (source.isFile() && !source.renameTo(new File(parent, destinationName))) {
            Log.w(LOG_TAG, "Renamed RAW but could not keep its " + description);
        }
    }

    @Override
    public void delete(Uri rawUri, String displayName) {
        File raw = new File(rawUri.getPath());
        if (raw.exists() && !raw.delete()) {
            throw new IllegalStateException("Could not delete the RAW file");
        }

        try {
            AndroidStorageContract.deleteSidecar(raw.getParentFile(), displayName);
        } catch (Exception error) {
            Log.w(LOG_TAG, "Deleted RAW but could not clean up its sidecar", error);
        }
        try {
            AndroidStorageContract.deleteAiDenoiseResult(raw.getParentFile(), displayName);
        } catch (Exception error) {
            Log.w(LOG_TAG, "Deleted RAW but could not clean up its AI-denoise result", error);
        }
        try {
            AndroidStorageContract.deletePeopleAnalysis(raw.getParentFile(), displayName);
        } catch (Exception error) {
            Log.w(LOG_TAG, "Deleted RAW but could not clean up its people analysis", error);
        }
    }

    @Override
    public void deleteEdits(Uri rawUri, String displayName) {
        File directory = new File(rawUri.getPath()).getParentFile();
        AndroidStorageContract.deleteSidecar(directory, displayName);
        AndroidStorageContract.deleteAiDenoiseResult(directory, displayName);
        AndroidStorageContract.deletePeopleAnalysis(directory, displayName);
    }

    @Override
    public InputStream openSidecar(Uri rawUri, String displayName) throws Exception {
        File sidecar = new File(
                new File(rawUri.getPath()).getParentFile(),
                AndroidStorageContract.sidecarDisplayName(displayName));
        return sidecar.isFile() ? new FileInputStream(sidecar) : null;
    }

    @Override
    public String publishSidecar(File cached, Uri rawUri, String displayName) throws Exception {
        return AndroidStorageContract.publishSidecarAtomically(
                cached,
                new File(rawUri.getPath()).getParentFile(),
                displayName,
                MAX_SIDECAR_BYTES);
    }

    // Native code reads and writes companions directly: they are too large to stage through the
    // cache directory like the sidecar, so fetching and publishing have nothing to do here.
    @Override
    public String companionPath(Uri rawUri, String displayName, String suffix) {
        return companionFile(rawUri, displayName, suffix).getAbsolutePath();
    }

    @Override
    public void fetchCompanion(Uri rawUri, String displayName, String suffix) {}

    @Override
    public void publishCompanion(Uri rawUri, String displayName, String suffix) {}

    @Override
    public void removeCompanion(Uri rawUri, String displayName, String suffix) {
        File companion = companionFile(rawUri, displayName, suffix);
        if (companion.exists() && !companion.delete()) {
            throw new IllegalStateException("Could not remove " + companion);
        }
    }

    @Override
    public boolean hasCompanion(Uri rawUri, String displayName, String suffix) {
        return companionFile(rawUri, displayName, suffix).isFile();
    }

    private static File companionFile(Uri rawUri, String displayName, String suffix) {
        return new File(
                new File(rawUri.getPath()).getParentFile(),
                AndroidStorageContract.companionDisplayName(displayName, suffix));
    }

    private File externalMediaRootDirectory() {
        File[] mediaDirectories = storage.getExternalMediaDirs();
        if (mediaDirectories != null) {
            for (File directory : mediaDirectories) {
                if (directory != null) {
                    return directory;
                }
            }
        }
        throw new IllegalStateException("Android shared media storage is unavailable");
    }

    private File rawLibraryDirectory() {
        File directory = AndroidStorageContract.rawLibraryDirectory(externalMediaRootDirectory());
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IllegalStateException("Could not create " + directory);
        }
        File noMedia = AndroidStorageContract.noMediaMarker(directory);
        try {
            if (!noMedia.exists() && !noMedia.createNewFile()) {
                Log.w(LOG_TAG, "Could not create .nomedia marker in " + directory);
            }
        } catch (Exception error) {
            Log.w(LOG_TAG, "Could not create .nomedia marker in " + directory, error);
        }
        return directory;
    }

    private File directoryOf(String folder) {
        try {
            File directory = AndroidStorageContract.libraryFolder(rawLibraryDirectory(), folder);
            if (!directory.isDirectory()) {
                throw new IllegalStateException("The selected RAW library folder no longer exists");
            }
            return directory;
        } catch (RuntimeException error) {
            throw error;
        } catch (Exception error) {
            throw new IllegalStateException("The selected RAW library folder is invalid", error);
        }
    }

    // The heap head is the record the old stable millisecond sort would drop first:
    // oldest mtime, then latest position in the original listFiles() result.
    private static final Comparator<RawLibraryCandidate> RAW_LIBRARY_CUTOFF_WORST_FIRST =
            (left, right) -> {
                int modifiedOrder = Long.compare(left.modifiedMillis, right.modifiedMillis);
                return modifiedOrder != 0
                        ? modifiedOrder
                        : Integer.compare(right.enumerationOrder, left.enumerationOrder);
            };

    private static final class RawLibraryCandidate {
        final File file;
        final long modifiedMillis;
        final int enumerationOrder;

        RawLibraryCandidate(File file, long modifiedMillis, int enumerationOrder) {
            this.file = file;
            this.modifiedMillis = modifiedMillis;
            this.enumerationOrder = enumerationOrder;
        }
    }
}
