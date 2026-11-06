package de.duecki.calibraw;

import android.app.Activity;
import android.content.ContentResolver;
import android.database.Cursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import android.util.Log;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileNotFoundException;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.SyncFailedException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeMap;

/**
 * A folder the user linked through the Storage Access Framework. Photos are worked on where they
 * are, so what is saved here reaches other devices that sync the folder.
 *
 * Photos are named by {@code calibraw-tree://{key}/{folder path}/{name}} instead of the provider's
 * document URI: a document URI does not reliably give its parent, which finding the sidecar needs,
 * and a provider may change a document's ID when it is renamed.
 *
 * The Storage Access Framework has no atomic replace, so a saved file is written beside its
 * destination, the old file moves aside, and the new one takes its name. A crash between the two
 * renames leaves the old sidecar under {@link AndroidStorageContract#linkedSwapOldName}, which the
 * next read restores.
 */
final class TreeLibraryStore implements LibraryStore {
    static final String SCHEME = "calibraw-tree";

    private static final String LOG_TAG = "CalibRaw";
    private static final String MIME_UNKNOWN = "application/octet-stream";
    private static final long MAX_COMPANION_BYTES = 8L * 1024L * 1024L * 1024L;
    private static final String MIRROR_DIRECTORY = "linked-companions";
    private static final String[] COLUMNS = {
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
            DocumentsContract.Document.COLUMN_LAST_MODIFIED,
            DocumentsContract.Document.COLUMN_FLAGS
    };

    private static final class Entry {
        final String documentId;
        final String displayName;
        final boolean directory;
        final long size;
        final long modifiedMillis;
        final int flags;

        Entry(
                String documentId,
                String displayName,
                boolean directory,
                long size,
                long modifiedMillis,
                int flags) {
            this.documentId = documentId;
            this.displayName = displayName;
            this.directory = directory;
            this.size = size;
            this.modifiedMillis = modifiedMillis;
            this.flags = flags;
        }

        boolean supportsRename() {
            return (flags & DocumentsContract.Document.FLAG_SUPPORTS_RENAME) != 0;
        }
    }

    /** A photo's place in the tree, from its verified URI. */
    private static final class Place {
        final String folder;
        final String name;

        Place(String folder, String name) {
            this.folder = folder;
            this.name = name;
        }
    }

    private final Activity storage;
    private final String key;
    private final Uri treeUri;
    private final String label;
    /** Held for a whole sidecar save, so two saves of one photo cannot share a temporary file. */
    private final Object sidecarWrites = new Object();
    // Guarded by this. Holding this also keeps a sidecar read out of a replace's two renames.
    private final Map<String, Map<String, Entry>> index = new HashMap<>();
    private final Map<String, Long> listedSignatures = new HashMap<>();
    private long writes;

    TreeLibraryStore(Activity storage, String key, Uri treeUri, String label) {
        this.storage = storage;
        this.key = key;
        this.treeUri = treeUri;
        this.label = label;
    }

    private ContentResolver resolver() {
        return storage.getContentResolver();
    }

    @Override
    public String location() {
        return label;
    }

    @Override
    public String listFolders() throws Exception {
        StringBuilder result = new StringBuilder();
        appendFolders(DocumentsContract.getTreeDocumentId(treeUri), "", 0, new int[]{0}, result);
        return result.toString();
    }

    private void appendFolders(
            String documentId,
            String path,
            int depth,
            int[] count,
            StringBuilder result) throws Exception {
        if (depth >= MAX_FOLDER_DEPTH || count[0] >= MAX_FOLDERS) {
            return;
        }
        List<Entry> folders = new ArrayList<>();
        for (Entry entry : query(documentId).values()) {
            if (entry.directory && isUsableFolderName(entry.displayName)) {
                folders.add(entry);
            }
        }
        Collections.sort(folders, (left, right) ->
                left.displayName.compareToIgnoreCase(right.displayName));
        for (Entry folder : folders) {
            if (count[0] >= MAX_FOLDERS) {
                return;
            }
            String relative = path.isEmpty() ? folder.displayName : path + "/" + folder.displayName;
            result.append(Uri.encode(relative)).append('\t')
                    .append(Uri.encode(folder.displayName)).append('\n');
            count[0]++;
            appendFolders(folder.documentId, relative, depth + 1, count, result);
        }
    }

    /** A folder {@link #normalizeFolder} accepts; "Trip " would be listed but never open. */
    private static boolean isUsableFolderName(String name) {
        try {
            return AndroidStorageContract.safeFolderName(name).equals(name);
        } catch (IllegalArgumentException unusable) {
            return false;
        }
    }

    @Override
    public ArrayList<Document> listDocuments(String folder, int limit) throws Exception {
        String normalized = normalizeFolder(folder);
        Map<String, Entry> entries = refresh(normalized);
        synchronized (this) {
            listedSignatures.put(normalized, signature(entries));
        }
        deleteStaleTemporaryFiles(entries);

        List<Entry> photos = new ArrayList<>();
        for (Entry entry : entries.values()) {
            // Sync tools hide their unfinished files behind a leading dot.
            if (!entry.directory
                    && !entry.displayName.startsWith(".")
                    && AndroidStorageContract.isSupportedImageName(entry.displayName)) {
                photos.add(entry);
            }
        }
        Collections.sort(photos, (left, right) ->
                Long.compare(right.modifiedMillis, left.modifiedMillis));
        ArrayList<Document> result = new ArrayList<>();
        for (Entry photo : photos.subList(0, Math.min(limit, photos.size()))) {
            result.add(new Document(
                    photoUri(normalized, photo.displayName).toString(),
                    photo.displayName,
                    displayPath(normalized, photo.displayName),
                    Math.max(0, photo.size),
                    Math.max(0, photo.modifiedMillis / 1000)));
        }
        return result;
    }

    /** Whether {@code folder} changed since it was last listed. False if it never was. */
    boolean changedSinceListing(String folder) throws Exception {
        String normalized = normalizeFolder(folder);
        Long listed;
        synchronized (this) {
            listed = listedSignatures.get(normalized);
        }
        return listed != null
                && listed != signature(query(folderDocumentId(normalized)));
    }

    private static long signature(Map<String, Entry> entries) {
        long signature = 17L;
        // Sorted, so the signature does not depend on the provider's order.
        for (Entry entry : new TreeMap<>(entries).values()) {
            signature = signature * 31L + entry.displayName.hashCode();
            signature = signature * 31L + entry.size;
            signature = signature * 31L + entry.modifiedMillis;
        }
        return signature;
    }

    private void deleteStaleTemporaryFiles(Map<String, Entry> entries) {
        long now = System.currentTimeMillis();
        for (Entry entry : entries.values()) {
            boolean stale = entry.modifiedMillis > 0L
                    && now >= entry.modifiedMillis
                    && now - entry.modifiedMillis >= StorageManager.STALE_TEMP_FILE_AGE_MS;
            if (stale && AndroidStorageContract.isLibraryTemporaryFileName(entry.displayName)) {
                deleteQuietly(documentUri(entry.documentId));
            }
        }
    }

    @Override
    public String normalizeFolder(String folder) {
        String relative = folder == null ? "" : folder.trim();
        if (relative.isEmpty()) {
            return "";
        }
        for (String component : relative.split("/", -1)) {
            if (component.isEmpty()
                    || !AndroidStorageContract.safeFolderName(component).equals(component)) {
                throw new IllegalArgumentException("The linked folder path is invalid");
            }
        }
        return relative;
    }

    @Override
    public String createFolder(String parent, String requestedName) throws Exception {
        String normalizedParent = normalizeFolder(parent);
        String name = AndroidStorageContract.safeFolderName(requestedName);
        if (refresh(normalizedParent).containsKey(name)) {
            throw new IllegalStateException(name + " already exists");
        }
        Uri created = DocumentsContract.createDocument(
                resolver(),
                documentUri(folderDocumentId(normalizedParent)),
                DocumentsContract.Document.MIME_TYPE_DIR,
                name);
        requireName(created, name);
        noteWrite(normalizedParent);
        return normalizedParent.isEmpty() ? name : normalizedParent + "/" + name;
    }

    @Override
    public void verify(Uri raw, String displayName) {
        place(raw, displayName);
    }

    private Place place(Uri raw, String displayName) {
        List<String> segments = raw.getPathSegments();
        if (!SCHEME.equals(raw.getScheme())
                || !key.equals(raw.getAuthority())
                || segments.isEmpty()
                || displayName == null
                || !displayName.equals(segments.get(segments.size() - 1))
                || displayName.indexOf('/') >= 0
                || displayName.indexOf('\0') >= 0
                || ".".equals(displayName)
                || "..".equals(displayName)) {
            throw new IllegalArgumentException("The photo is outside this linked folder");
        }
        String folder = normalizeFolder(
                String.join("/", segments.subList(0, segments.size() - 1)));
        return new Place(folder, displayName);
    }

    private Uri photoUri(String folder, String name) {
        Uri.Builder builder = new Uri.Builder().scheme(SCHEME).authority(key);
        if (!folder.isEmpty()) {
            for (String component : folder.split("/", -1)) {
                builder.appendPath(component);
            }
        }
        return builder.appendPath(name).build();
    }

    private String displayPath(String folder, String name) {
        return folder.isEmpty() ? label + "/" + name : label + "/" + folder + "/" + name;
    }

    @Override
    public ParcelFileDescriptor openDescriptor(Uri raw) throws Exception {
        Place place = place(raw, raw.getLastPathSegment());
        try {
            return resolver().openFileDescriptor(photoDocumentUri(place, false), "r");
        } catch (FileNotFoundException staleId) {
            return resolver().openFileDescriptor(photoDocumentUri(place, true), "r");
        }
    }

    @Override
    public InputStream openInput(Uri raw) throws Exception {
        Place place = place(raw, raw.getLastPathSegment());
        try {
            return resolver().openInputStream(photoDocumentUri(place, false));
        } catch (FileNotFoundException staleId) {
            return resolver().openInputStream(photoDocumentUri(place, true));
        }
    }

    private Uri photoDocumentUri(Place place, boolean fresh) throws Exception {
        Entry entry = (fresh ? refresh(place.folder) : entries(place.folder)).get(place.name);
        if (entry == null && !fresh) {
            entry = refresh(place.folder).get(place.name);
        }
        if (entry == null) {
            throw new FileNotFoundException(place.name + " is no longer in " + label);
        }
        return documentUri(entry.documentId);
    }

    @Override
    public Stored store(InputStream input, String requestedName, String folder) throws Exception {
        String normalized = normalizeFolder(folder);
        Map<String, Entry> entries = refresh(normalized);
        String name = uniqueName(entries, AndroidStorageContract.safeRawName(requestedName));
        Uri parent = documentUri(folderDocumentId(normalized));

        Uri target = createExact(parent, AndroidStorageContract.importPartialName(name));
        boolean renameToFinish = queryEntry(target).supportsRename();
        if (!renameToFinish) {
            deleteQuietly(target);
            target = createExact(parent, name);
        }
        boolean completed = false;
        try {
            try (ParcelFileDescriptor descriptor = resolver().openFileDescriptor(target, "w");
                    FileOutputStream output = new FileOutputStream(descriptor.getFileDescriptor())) {
                BoundedStreams.copy(
                        input,
                        output,
                        StorageManager.MAX_RAW_IMPORT_BYTES,
                        StorageManager.storageLimitMessage(StorageManager.MAX_RAW_IMPORT_BYTES));
                syncIfSupported(output);
            }
            if (renameToFinish) {
                target = renameExact(target, name);
            }
            completed = true;
            return new Stored(photoUri(normalized, name), name);
        } finally {
            if (!completed) {
                deleteQuietly(target);
            }
            noteWrite(normalized);
        }
    }

    private static String uniqueName(Map<String, Entry> entries, String name) {
        if (!entries.containsKey(name)) {
            return name;
        }
        int dot = name.lastIndexOf('.');
        String stem = dot > 0 ? name.substring(0, dot) : name;
        String extension = dot > 0 ? name.substring(dot) : "";
        for (int suffix = 1; ; suffix++) {
            String candidate = stem + "-" + suffix + extension;
            if (!entries.containsKey(candidate)) {
                return candidate;
            }
        }
    }

    @Override
    public Uri rename(Uri raw, String displayName, String requestedName) throws Exception {
        Place place = place(raw, displayName);
        synchronized (sidecarWrites) {
            Map<String, Entry> entries = refresh(place.folder);
            String sidecar = AndroidStorageContract.sidecarDisplayName(displayName);
            String destinationSidecar = AndroidStorageContract.sidecarDisplayName(requestedName);
            if (entries.containsKey(requestedName) || entries.containsKey(destinationSidecar)) {
                throw new IllegalStateException(requestedName + " already exists");
            }
            Entry photo = entries.get(displayName);
            if (photo == null) {
                throw new FileNotFoundException(displayName + " is no longer in " + label);
            }
            synchronized (this) {
                Uri renamed = renameExact(documentUri(photo.documentId), requestedName);
                Entry sidecarEntry = entries.get(sidecar);
                if (sidecarEntry != null) {
                    try {
                        renameExact(documentUri(sidecarEntry.documentId), destinationSidecar);
                    } catch (Exception error) {
                        renameExact(renamed, displayName);
                        throw error;
                    }
                }
                for (String suffix : AndroidStorageContract.REGENERATED_COMPANION_SUFFIXES) {
                    // The local copy is named after the old URI, which no longer reaches it.
                    deleteMirror(raw, suffix);
                    Entry companion = entries.get(
                            AndroidStorageContract.companionDisplayName(displayName, suffix));
                    if (companion == null) {
                        continue;
                    }
                    // The app can produce this file again, so a failed move only costs that.
                    try {
                        renameExact(
                                documentUri(companion.documentId),
                                AndroidStorageContract.companionDisplayName(requestedName, suffix));
                    } catch (Exception error) {
                        Log.w(LOG_TAG, "Renamed photo but could not keep " + suffix, error);
                    }
                }
                noteWrite(place.folder);
            }
            return photoUri(place.folder, requestedName);
        }
    }

    @Override
    public void delete(Uri raw, String displayName) throws Exception {
        Place place = place(raw, displayName);
        synchronized (sidecarWrites) {
            Map<String, Entry> entries = refresh(place.folder);
            synchronized (this) {
                Entry photo = entries.get(displayName);
                if (photo != null
                        && !DocumentsContract.deleteDocument(
                                resolver(), documentUri(photo.documentId))) {
                    throw new IllegalStateException("Could not delete the photo");
                }
                try {
                    deleteEditsFrom(raw, entries, displayName);
                } catch (Exception error) {
                    Log.w(LOG_TAG, "Deleted photo but could not clean up its edits", error);
                }
                noteWrite(place.folder);
            }
        }
    }

    @Override
    public void deleteEdits(Uri raw, String displayName) throws Exception {
        Place place = place(raw, displayName);
        synchronized (sidecarWrites) {
            Map<String, Entry> entries = refresh(place.folder);
            synchronized (this) {
                deleteEditsFrom(raw, entries, displayName);
                noteWrite(place.folder);
            }
        }
    }

    private void deleteEditsFrom(Uri raw, Map<String, Entry> entries, String displayName)
            throws Exception {
        deleteIfPresent(entries.get(AndroidStorageContract.sidecarDisplayName(displayName)));
        for (String suffix : AndroidStorageContract.REGENERATED_COMPANION_SUFFIXES) {
            deleteIfPresent(
                    entries.get(AndroidStorageContract.companionDisplayName(displayName, suffix)));
            deleteMirror(raw, suffix);
        }
    }

    private void deleteIfPresent(Entry entry) throws Exception {
        if (entry != null
                && !DocumentsContract.deleteDocument(resolver(), documentUri(entry.documentId))) {
            throw new IllegalStateException("Could not delete " + entry.displayName);
        }
    }

    @Override
    public synchronized InputStream openSidecar(Uri raw, String displayName) throws Exception {
        Place place = place(raw, displayName);
        String name = AndroidStorageContract.sidecarDisplayName(displayName);
        Entry cached = entries(place.folder).get(name);
        if (cached != null) {
            try {
                return resolver().openInputStream(documentUri(cached.documentId));
            } catch (FileNotFoundException staleId) {
                // Sync replaced or removed it since the folder was listed.
            }
        }
        // The index may predate a sidecar sync brought since. Opening the photo without it
        // would let the next save overwrite the edits made on another device.
        Map<String, Entry> entries = refresh(place.folder);
        Entry sidecar = entries.get(name);
        if (sidecar == null) {
            Entry interrupted = entries.get(AndroidStorageContract.linkedSwapOldName(name));
            if (interrupted == null) {
                return null;
            }
            renameExact(documentUri(interrupted.documentId), name);
            noteWrite(place.folder);
            sidecar = refresh(place.folder).get(name);
            if (sidecar == null) {
                return null;
            }
        }
        return resolver().openInputStream(documentUri(sidecar.documentId));
    }

    @Override
    public String publishSidecar(File cached, Uri raw, String displayName) throws Exception {
        Place place = place(raw, displayName);
        String name = AndroidStorageContract.sidecarDisplayName(displayName);
        synchronized (sidecarWrites) {
            replaceDocument(place.folder, name, cached);
        }
        return displayPath(place.folder, name);
    }

    @Override
    public String companionPath(Uri raw, String displayName, String suffix) throws Exception {
        place(raw, displayName);
        return mirror(raw, suffix).getAbsolutePath();
    }

    @Override
    public void fetchCompanion(Uri raw, String displayName, String suffix) throws Exception {
        Place place = place(raw, displayName);
        File mirror = mirror(raw, suffix);
        // Fresh: sync may have brought or replaced the result since the folder was listed, and
        // its size and time decide whether the local copy is still it.
        Entry stored = refresh(place.folder).get(
                AndroidStorageContract.companionDisplayName(displayName, suffix));
        if (stored == null) {
            deleteLocal(mirror);
            return;
        }
        if (mirror.isFile()
                && mirror.length() == stored.size
                && mirror.lastModified() == stored.modifiedMillis) {
            return;
        }
        File partial = new File(mirror.getPath() + ".part");
        boolean completed = false;
        try {
            try (InputStream input =
                            resolver().openInputStream(documentUri(stored.documentId));
                    FileOutputStream output = new FileOutputStream(partial)) {
                if (input == null) {
                    throw new IllegalStateException("Android storage returned no stream");
                }
                BoundedStreams.copy(
                        input,
                        output,
                        MAX_COMPANION_BYTES,
                        StorageManager.storageLimitMessage(MAX_COMPANION_BYTES));
                output.getFD().sync();
            }
            if (!partial.renameTo(mirror)) {
                throw new IllegalStateException("Could not place " + mirror);
            }
            completed = true;
            //noinspection ResultOfMethodCallIgnored -- a mismatch only copies it again next time
            mirror.setLastModified(stored.modifiedMillis);
        } finally {
            if (!completed) {
                deleteLocal(partial);
            }
        }
    }

    @Override
    public void publishCompanion(Uri raw, String displayName, String suffix) throws Exception {
        Place place = place(raw, displayName);
        File mirror = mirror(raw, suffix);
        if (!mirror.isFile()) {
            return;
        }
        String name = AndroidStorageContract.companionDisplayName(displayName, suffix);
        Entry stored = entries(place.folder).get(name);
        // What was just restored from the folder needs no copying back.
        if (stored != null
                && stored.size == mirror.length()
                && stored.modifiedMillis == mirror.lastModified()) {
            return;
        }
        replaceDocument(place.folder, name, mirror);
        Entry published = entries(place.folder).get(name);
        if (published != null) {
            //noinspection ResultOfMethodCallIgnored -- a mismatch only copies it again next time
            mirror.setLastModified(published.modifiedMillis);
        }
    }

    @Override
    public void removeCompanion(Uri raw, String displayName, String suffix) throws Exception {
        Place place = place(raw, displayName);
        deleteMirror(raw, suffix);
        Entry stored = entries(place.folder).get(
                AndroidStorageContract.companionDisplayName(displayName, suffix));
        if (stored != null) {
            synchronized (this) {
                deleteIfPresent(stored);
                noteWrite(place.folder);
            }
        }
    }

    @Override
    public boolean hasCompanion(Uri raw, String displayName, String suffix) throws Exception {
        Place place = place(raw, displayName);
        return entries(place.folder).containsKey(
                AndroidStorageContract.companionDisplayName(displayName, suffix));
    }

    /** Native code keeps companions in a local file, since it cannot write a document by path. */
    private File mirror(Uri raw, String suffix) throws Exception {
        File directory = new File(storage.getCacheDir(), MIRROR_DIRECTORY);
        if (!directory.isDirectory() && !directory.mkdirs()) {
            throw new IllegalStateException("Could not create " + directory);
        }
        return new File(directory, AndroidStorageContract.sha256Hex(raw.toString()) + suffix);
    }

    private void deleteMirror(Uri raw, String suffix) {
        try {
            deleteLocal(mirror(raw, suffix));
        } catch (Exception error) {
            Log.w(LOG_TAG, "Could not remove a local companion copy", error);
        }
    }

    private static void deleteLocal(File file) {
        if (!file.delete() && file.exists()) {
            Log.w(LOG_TAG, "Could not delete " + file);
        }
    }

    /** Replaces {@code name} in {@code folder} with the contents of {@code local}. */
    private void replaceDocument(String folder, String name, File local) throws Exception {
        // Fresh, because a provider numbers a name that is already taken instead of refusing it,
        // and sync may have added the file since the folder was last listed.
        Map<String, Entry> entries = refresh(folder);
        Uri parent = documentUri(folderDocumentId(folder));
        String partName = AndroidStorageContract.linkedSwapPartName(name);
        String oldName = AndroidStorageContract.linkedSwapOldName(name);
        Entry existing = entries.get(name);
        Entry interrupted = entries.get(oldName);

        Entry leftover = entries.get(partName);
        if (leftover != null) {
            deleteIfPresent(leftover);
        }
        Uri part = createExact(parent, partName);
        boolean published = false;
        try {
            if (!queryEntry(part).supportsRename()) {
                deleteQuietly(part);
                overwrite(parent, name, existing, local);
                published = true;
                return;
            }
            writeLocal(local, part, "w");
            synchronized (this) {
                Uri moved = null;
                if (existing != null) {
                    if (interrupted != null) {
                        deleteIfPresent(interrupted);
                    }
                    moved = renameExact(documentUri(existing.documentId), oldName);
                }
                try {
                    renameExact(part, name);
                    published = true;
                } catch (Exception error) {
                    if (moved != null) {
                        renameExact(moved, name);
                    }
                    throw error;
                }
                if (moved != null) {
                    deleteQuietly(moved);
                } else if (interrupted != null) {
                    deleteQuietly(documentUri(interrupted.documentId));
                }
            }
        } finally {
            if (!published) {
                deleteQuietly(part);
            }
            noteWrite(folder);
        }
    }

    /** For providers without rename: this cannot keep the old file through a failed write. */
    private void overwrite(Uri parent, String name, Entry existing, File local) throws Exception {
        Uri target = existing != null
                ? documentUri(existing.documentId)
                : createExact(parent, name);
        writeLocal(local, target, existing != null ? "rwt" : "w");
        long written = queryEntry(target).size;
        if (written >= 0 && written != local.length()) {
            throw new IllegalStateException(
                    "Android storage kept " + written + " of " + local.length() + " bytes of "
                            + name);
        }
    }

    private void writeLocal(File local, Uri target, String mode) throws Exception {
        try (ParcelFileDescriptor descriptor = resolver().openFileDescriptor(target, mode);
                FileInputStream input = new FileInputStream(local);
                FileOutputStream output = new FileOutputStream(descriptor.getFileDescriptor())) {
            BoundedStreams.copy(
                    input,
                    output,
                    MAX_COMPANION_BYTES,
                    StorageManager.storageLimitMessage(MAX_COMPANION_BYTES));
            syncIfSupported(output);
        }
    }

    /** A provider's pipe cannot sync; closing it reports whether the provider stored the data. */
    private static void syncIfSupported(FileOutputStream output) throws Exception {
        try {
            output.getFD().sync();
        } catch (SyncFailedException unsupported) {
            Log.d(LOG_TAG, "This document does not support sync");
        }
    }

    /** Creates {@code name}; a provider that would store it under another name is refused. */
    private Uri createExact(Uri parent, String name) throws Exception {
        Uri created = DocumentsContract.createDocument(resolver(), parent, MIME_UNKNOWN, name);
        if (created == null) {
            throw new IllegalStateException("Android storage could not create " + name);
        }
        try {
            requireName(created, name);
        } catch (Exception error) {
            deleteQuietly(created);
            throw error;
        }
        return created;
    }

    private Uri renameExact(Uri document, String name) throws Exception {
        Uri renamed = DocumentsContract.renameDocument(resolver(), document, name);
        // A provider that keeps the document's ID may report no new URI.
        Uri current = renamed != null ? renamed : document;
        requireName(current, name);
        return current;
    }

    private void requireName(Uri document, String name) throws Exception {
        if (document == null) {
            throw new IllegalStateException("Android storage could not create " + name);
        }
        String actual = queryEntry(document).displayName;
        if (!name.equals(actual)) {
            throw new IllegalStateException(
                    "Android storage saved " + name + " as " + actual
                            + ", which CalibRaw would not find again");
        }
    }

    private void deleteQuietly(Uri document) {
        try {
            DocumentsContract.deleteDocument(resolver(), document);
        } catch (Exception error) {
            Log.w(LOG_TAG, "Could not delete a temporary file in " + label, error);
        }
    }

    private Uri documentUri(String documentId) {
        return DocumentsContract.buildDocumentUriUsingTree(treeUri, documentId);
    }

    private String folderDocumentId(String folder) throws Exception {
        String documentId = DocumentsContract.getTreeDocumentId(treeUri);
        if (folder.isEmpty()) {
            return documentId;
        }
        String walked = "";
        for (String component : folder.split("/", -1)) {
            Entry child = entries(walked).get(component);
            if (child == null) {
                child = refresh(walked).get(component);
            }
            if (child == null || !child.directory) {
                throw new FileNotFoundException(folder + " is no longer in " + label);
            }
            documentId = child.documentId;
            walked = walked.isEmpty() ? component : walked + "/" + component;
        }
        return documentId;
    }

    private Map<String, Entry> entries(String folder) throws Exception {
        synchronized (this) {
            Map<String, Entry> known = index.get(folder);
            if (known != null) {
                return known;
            }
        }
        return refresh(folder);
    }

    /** Lists {@code folder} now. A write during the listing keeps the result out of the index. */
    private Map<String, Entry> refresh(String folder) throws Exception {
        long writesBefore;
        synchronized (this) {
            writesBefore = writes;
        }
        Map<String, Entry> entries = query(folderDocumentId(folder));
        synchronized (this) {
            if (writes == writesBefore) {
                index.put(folder, entries);
            }
        }
        return entries;
    }

    private synchronized void noteWrite(String folder) {
        writes++;
        index.remove(folder);
    }

    private Map<String, Entry> query(String parentDocumentId) throws Exception {
        Uri children =
                DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, parentDocumentId);
        Map<String, Entry> entries = new HashMap<>();
        try (Cursor cursor = resolver().query(children, COLUMNS, null, null, null)) {
            if (cursor == null) {
                throw new IllegalStateException(
                        String.format(Locale.ROOT, "Android storage could not list %s", label));
            }
            while (cursor.moveToNext()) {
                Entry entry = readEntry(cursor);
                if (entry != null) {
                    entries.put(entry.displayName, entry);
                }
            }
        }
        return entries;
    }

    private Entry queryEntry(Uri document) throws Exception {
        try (Cursor cursor = resolver().query(document, COLUMNS, null, null, null)) {
            Entry entry = cursor != null && cursor.moveToFirst() ? readEntry(cursor) : null;
            if (entry == null) {
                throw new FileNotFoundException("Android storage could not find " + document);
            }
            return entry;
        }
    }

    private static Entry readEntry(Cursor cursor) {
        String documentId = cursor.getString(0);
        String name = cursor.getString(1);
        if (documentId == null || name == null) {
            return null;
        }
        return new Entry(
                documentId,
                name,
                DocumentsContract.Document.MIME_TYPE_DIR.equals(cursor.getString(2)),
                cursor.isNull(3) ? -1L : cursor.getLong(3),
                cursor.isNull(4) ? 0L : cursor.getLong(4),
                cursor.isNull(5) ? 0 : cursor.getInt(5));
    }
}
