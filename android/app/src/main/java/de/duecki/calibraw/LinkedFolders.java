package de.duecki.calibraw;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.UriPermission;
import android.database.Cursor;
import android.net.Uri;
import android.provider.DocumentsContract;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * The folders the user linked. Android's persisted permissions are the list: a grant the user
 * revoked, or one lost with a reinstall, is simply no longer linked. Labels are remembered from
 * linking, since asking a provider for one could wait on a drive or network that is gone.
 */
final class LinkedFolders {
    private static final String LABELS = "calibraw-linked-folders";
    private static final String PICKER_URI_KEY = "linked-folder-tree-uri";
    private static final int ACCESS =
            Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION;

    private final Activity storage;
    private final PickerLocationStore pickerLocations;
    // Guarded by this.
    private final Map<String, TreeLibraryStore> stores = new HashMap<>();

    LinkedFolders(Activity storage) {
        this.storage = storage;
        this.pickerLocations = new PickerLocationStore(storage);
    }

    Intent createPickerIntent() {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE);
        intent.addFlags(ACCESS
                | Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
                | Intent.FLAG_GRANT_PREFIX_URI_PERMISSION);
        Uri initialUri = pickerLocations.readContentUri(PICKER_URI_KEY);
        if (initialUri != null) {
            intent.putExtra(DocumentsContract.EXTRA_INITIAL_URI, initialUri);
        }
        return intent;
    }

    /** Keeps access to the picked tree and returns its key. Reaches storage. */
    String link(Uri treeUri) throws Exception {
        String key = keyOf(treeUri);
        boolean wasLinked = isGranted(key);
        storage.getContentResolver().takePersistableUriPermission(treeUri, ACCESS);
        boolean linked = false;
        try {
            Uri document = DocumentsContract.buildDocumentUriUsingTree(
                    treeUri, DocumentsContract.getTreeDocumentId(treeUri));
            String label = fallbackLabel(treeUri);
            int flags = 0;
            try (Cursor cursor = storage.getContentResolver().query(
                    document,
                    new String[]{
                            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                            DocumentsContract.Document.COLUMN_FLAGS
                    },
                    null,
                    null,
                    null)) {
                if (cursor != null && cursor.moveToFirst()) {
                    String name = cursor.getString(0);
                    if (name != null && !name.isEmpty()) {
                        label = name;
                    }
                    flags = cursor.getInt(1);
                }
            }
            // Edits are saved beside the photos, so a folder that takes no new files cannot work.
            if ((flags & DocumentsContract.Document.FLAG_DIR_SUPPORTS_CREATE) == 0) {
                throw new IllegalArgumentException(
                        "CalibRaw cannot save edits in this folder. Choose one it can write to.");
            }
            labels().edit().putString(key, label).apply();
            // The picker opens where a document is, so it is given the folder, not the tree.
            pickerLocations.writeContentUri(PICKER_URI_KEY, document);
            linked = true;
            return key;
        } finally {
            // The grant is the list of linked folders, so one that failed must not stay. A
            // folder linked before keeps its grant; its store, and the locks it holds, stay too.
            if (!linked && !wasLinked) {
                storage.getContentResolver().releasePersistableUriPermission(treeUri, ACCESS);
            }
        }
    }

    private boolean isGranted(String key) {
        for (Uri tree : grantedTrees()) {
            if (key.equals(keyOf(tree))) {
                return true;
            }
        }
        return false;
    }

    void unlink(String key) {
        for (UriPermission permission :
                storage.getContentResolver().getPersistedUriPermissions()) {
            if (key.equals(keyOf(permission.getUri()))) {
                storage.getContentResolver()
                        .releasePersistableUriPermission(permission.getUri(), ACCESS);
            }
        }
        labels().edit().remove(key).apply();
        synchronized (this) {
            stores.remove(key);
        }
    }

    /** The linked folder, or null when it is no longer linked. Reaches no provider. */
    synchronized TreeLibraryStore store(String key) {
        TreeLibraryStore known = stores.get(key);
        for (Uri tree : grantedTrees()) {
            if (!key.equals(keyOf(tree))) {
                continue;
            }
            if (known != null) {
                return known;
            }
            String label = labels().getString(key, fallbackLabel(tree));
            TreeLibraryStore created = new TreeLibraryStore(storage, key, tree, label);
            stores.put(key, created);
            return created;
        }
        stores.remove(key);
        return null;
    }

    /** "{encoded key}\t{encoded label}\n" lines. Reaches no provider. */
    String encodedList() {
        List<Uri> trees = grantedTrees();
        Map<String, Integer> labelUses = new HashMap<>();
        for (Uri tree : trees) {
            String label = labels().getString(keyOf(tree), fallbackLabel(tree));
            Integer uses = labelUses.get(label);
            labelUses.put(label, uses == null ? 1 : uses + 1);
        }
        StringBuilder result = new StringBuilder();
        for (Uri tree : trees) {
            String key = keyOf(tree);
            String label = labels().getString(key, fallbackLabel(tree));
            Integer uses = labelUses.get(label);
            // Two folders called "Photos" are told apart by where they are.
            String shown = uses != null && uses > 1 ? distinctLabel(tree) : label;
            result.append(Uri.encode(key)).append('\t')
                    .append(Uri.encode(shown)).append('\n');
        }
        return result.toString();
    }

    private List<Uri> grantedTrees() {
        List<Uri> trees = new ArrayList<>();
        for (UriPermission permission :
                storage.getContentResolver().getPersistedUriPermissions()) {
            Uri uri = permission.getUri();
            if (permission.isReadPermission()
                    && permission.isWritePermission()
                    && DocumentsContract.isTreeUri(uri)) {
                trees.add(uri);
            }
        }
        return trees;
    }

    private SharedPreferences labels() {
        return storage.getSharedPreferences(LABELS, Context.MODE_PRIVATE);
    }

    private static String keyOf(Uri tree) {
        try {
            return AndroidStorageContract.sha256Hex(tree.toString());
        } catch (Exception impossible) {
            throw new IllegalStateException("SHA-256 is unavailable", impossible);
        }
    }

    /** "primary:Pictures/Sync" and "1A2B-3C4D:Sync" become "Sync". */
    private static String fallbackLabel(Uri tree) {
        String path = distinctLabel(tree);
        String name = path.substring(Math.max(path.lastIndexOf('/'), path.indexOf(':')) + 1);
        return name.isEmpty() ? path : name;
    }

    /** "primary:Pictures/Sync" becomes "Pictures/Sync"; another volume keeps its name. */
    private static String distinctLabel(Uri tree) {
        String id = DocumentsContract.getTreeDocumentId(tree);
        return id.startsWith("primary:") ? id.substring("primary:".length()) : id;
    }
}
