package de.duecki.calibraw;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import java.io.File;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public final class AndroidStorageContractTest {
    @Rule public final TemporaryFolder temporaryFolder = new TemporaryFolder();

    @Test
    public void namesAndRawFileIdentityFollowTheStorageContract() throws Exception {
        File root = temporaryFolder.getRoot();
        File media = new File(root, "media");
        File library = AndroidStorageContract.rawLibraryDirectory(media);

        assertEquals(".library", library.getName());
        assertEquals(".nomedia", AndroidStorageContract.noMediaMarker(library).getName());

        assertEquals("a_b_c.dng", AndroidStorageContract.safeRawName("a/b\\c.dng"));
        assertTrue(AndroidStorageContract.isRawName("capture.DNG"));
        assertTrue(AndroidStorageContract.isRawName("capture.raf"));
        assertTrue(AndroidStorageContract.isRawName("rendered.TIF"));
        assertTrue(AndroidStorageContract.isRawName("rendered.tiff"));
        assertFalse(AndroidStorageContract.isRawName("capture.jpg"));
        assertEquals("capture.dng.calibraw", AndroidStorageContract.sidecarDisplayName("capture.dng"));
        assertEquals("rendered.tif.calibraw", AndroidStorageContract.sidecarDisplayName("rendered.tif"));
        assertEquals(
                ".calibraw-import-capture.dng.part",
                AndroidStorageContract.importPartialName("capture.dng"));
        assertTrue(library.mkdirs());
        File trip = new File(library, "2026/Trip");
        assertTrue(trip.mkdirs());

        assertEquals("Trip", AndroidStorageContract.safeFolderName("Trip"));
        assertEquals(
                trip.getCanonicalFile(),
                AndroidStorageContract.libraryFolder(library, "2026/Trip"));
        assertEquals(
                "2026/Trip",
                AndroidStorageContract.relativeLibraryFolder(library, trip));

        File currentRaw = new File(library, "capture.dng");
        File nestedRaw = new File(trip, "trip.dng");
        Files.write(currentRaw.toPath(), new byte[] {1});
        Files.write(nestedRaw.toPath(), new byte[] {2});

        assertTrue(AndroidStorageContract.isAllowedRawFile(
                currentRaw, "capture.dng", library));
        assertTrue(AndroidStorageContract.isAllowedRawFile(
                nestedRaw, "trip.dng", library));
        assertFalse(AndroidStorageContract.isAllowedRawFile(
                currentRaw, "renamed.dng", library));

        File outside = new File(root, "capture.dng");
        Files.write(outside.toPath(), new byte[] {3});
        assertFalse(AndroidStorageContract.isAllowedRawFile(
                outside, "capture.dng", library));

        try {
            AndroidStorageContract.libraryFolder(library, "../outside");
            fail("folder traversal should be rejected");
        } catch (IllegalArgumentException expected) {
        }
    }

    @Test
    public void generatedStorageNamesStayWithinFilesystemLimits() {
        String unicodeRaw = "😀".repeat(100) + ".dng";
        String rawName = AndroidStorageContract.safeRawName(unicodeRaw);
        assertTrue(rawName.endsWith(".dng"));
        assertTrue(rawName.getBytes(StandardCharsets.UTF_8).length <= AndroidStorageContract.MAX_RAW_NAME_BYTES);
        assertTrue(AndroidStorageContract.importPartialName(rawName)
                .getBytes(StandardCharsets.UTF_8).length <= 255);

        String exportName = AndroidStorageContract.safeImageName(
                "x".repeat(400) + ".PNG", "image/png");
        assertTrue(exportName.endsWith(".PNG"));
        assertTrue(exportName.getBytes(StandardCharsets.UTF_8).length <= AndroidStorageContract.MAX_EXPORT_NAME_BYTES);

        String profileName = AndroidStorageContract.truncateUtf8PreservingExtension(
                "é".repeat(200) + ".dcp", 180);
        assertTrue(profileName.endsWith(".dcp"));
        assertTrue(profileName.getBytes(StandardCharsets.UTF_8).length <= 180);
    }

    @Test
    public void temporaryLibraryFileClassifierCoversOwnedPartialFilesOnly() {
        assertTrue(AndroidStorageContract.isLibraryTemporaryFileName(
                ".calibraw-import-capture.dng.part"));
        assertTrue(AndroidStorageContract.isLibraryTemporaryFileName(
                ".calibraw-sidecar-123.part"));
        assertFalse(AndroidStorageContract.isLibraryTemporaryFileName("capture.dng"));
        assertFalse(AndroidStorageContract.isLibraryTemporaryFileName("notes.part"));
        assertFalse(AndroidStorageContract.isLibraryTemporaryFileName(
                ".calibraw-import-capture.dng"));
    }

    @Test
    public void sidecarsPublishAtomicallyAndRespectTheSizeLimit() throws Exception {
        File root = temporaryFolder.getRoot();
        File library = new File(root, ".library");
        assertTrue(library.mkdirs());

        File staged = new File(root, "staged.calibraw");
        byte[] payload = "new-sidecar".getBytes(StandardCharsets.UTF_8);
        Files.write(staged.toPath(), payload);

        File destination = new File(AndroidStorageContract.publishSidecarAtomically(
                staged, library, "capture.dng", payload.length));
        assertEquals(library.getCanonicalFile(), destination.getParentFile().getCanonicalFile());
        assertArrayEquals(payload, Files.readAllBytes(destination.toPath()));

        File[] partials = library.listFiles((directory, name) -> name.endsWith(".part"));
        assertTrue(partials != null && partials.length == 0);

        byte[] oldPayload = "old-sidecar".getBytes(StandardCharsets.UTF_8);
        Files.write(destination.toPath(), oldPayload);
        File oversized = new File(root, "oversized.calibraw");
        Files.write(oversized.toPath(), new byte[] {1, 2, 3, 4});

        try {
            AndroidStorageContract.publishSidecarAtomically(
                    oversized, library, "capture.dng", 3);
            fail("oversized sidecar should be rejected");
        } catch (IllegalStateException expected) {
        }
        assertArrayEquals(oldPayload, Files.readAllBytes(destination.toPath()));

        AndroidStorageContract.deleteSidecar(library, "capture.dng");
        assertFalse(destination.exists());
    }

    @Test
    public void exportNamingFollowsTheStorageContract() {
        assertEquals("Pictures/CalibRaw", AndroidStorageContract.exportRelativePath("Pictures"));
        assertEquals(
                "Pictures/CalibRaw/edit.png",
                AndroidStorageContract.exportLocation("Pictures", "edit.png"));
        assertEquals("image/jpeg", AndroidStorageContract.normalizeExportMimeType("IMAGE/JPEG"));
        assertEquals("image/png", AndroidStorageContract.normalizeExportMimeType("image/webp"));
        assertEquals("image/jxl", AndroidStorageContract.normalizeExportMimeType("IMAGE/JXL"));
        assertEquals(
                "summer_edit.png",
                AndroidStorageContract.safeImageName("summer edit", "image/png"));
        assertEquals("summer_edit.jxl", AndroidStorageContract.safeImageName("summer edit", "image/jxl"));
    }

    @Test
    public void thumbnailTrimEnforcesThePersistentCacheByteBudget() throws Exception {
        File directory = temporaryFolder.newFolder("thumbnail-cache");
        File oldest = new File(directory, "oldest.raw.jpg");
        File middle = new File(directory, "middle.developed.jpg");
        File newest = new File(directory, "newest.raw.jpg");
        writeSparseFile(oldest, 50L * 1024L * 1024L, 1_000L);
        writeSparseFile(middle, 50L * 1024L * 1024L, 2_000L);
        writeSparseFile(newest, 50L * 1024L * 1024L, 3_000L);
        File middleFingerprint = new File(middle.getPath() + ".fingerprint");
        Files.write(middleFingerprint.toPath(), new byte[] {1, 2, 3, 4});
        assertTrue(middleFingerprint.setLastModified(2_000L));

        ThumbnailCache.trim(directory);

        assertFalse(oldest.exists());
        assertTrue(middle.exists());
        assertTrue(newest.exists());
        assertTrue(directoryBytes(directory) <= 128L * 1024L * 1024L);
    }

    @Test
    public void thumbnailPathLookupTouchesHitsWithoutTrimmingTheWholeCache() throws Exception {
        File directory = temporaryFolder.newFolder("thumbnail-cache-lookup");
        String identity = "developed\ncontent://library/photo/1";
        File cached = ThumbnailCache.pathInDirectory(directory, identity, ".developed.jpg");
        File oldest = new File(directory, "oldest.raw.jpg");
        File middle = new File(directory, "middle.raw.jpg");
        writeSparseFile(oldest, 50L * 1024L * 1024L, 1_000L);
        writeSparseFile(middle, 50L * 1024L * 1024L, 2_000L);
        writeSparseFile(cached, 50L * 1024L * 1024L, 3_000L);

        File lookedUp = ThumbnailCache.pathInDirectory(directory, identity, ".developed.jpg");

        assertEquals(cached.getCanonicalFile(), lookedUp.getCanonicalFile());
        assertTrue(cached.lastModified() > 3_000L);
        assertTrue(oldest.exists());
        assertTrue(middle.exists());
        assertTrue(directoryBytes(directory) > 128L * 1024L * 1024L);

        ThumbnailCache.trim(directory);

        assertFalse(oldest.exists());
        assertTrue(cached.exists());
        assertTrue(directoryBytes(directory) <= 128L * 1024L * 1024L);
    }

    @Test
    public void boundedStreamsEnforceLimitsAndRecoverFromZeroProgressReads() throws Exception {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        long copied = BoundedStreams.copy(
                new ZeroProgressInputStream("raw".getBytes(StandardCharsets.UTF_8)),
                output,
                3,
                "too large");

        assertEquals(3, copied);
        assertArrayEquals("raw".getBytes(StandardCharsets.UTF_8), output.toByteArray());

        try {
            BoundedStreams.copy(
                    new ByteArrayInputStream("oversized".getBytes(StandardCharsets.UTF_8)),
                    new ByteArrayOutputStream(),
                    4,
                    "too large");
            fail("copy should reject input beyond the explicit bound");
        } catch (StorageLimitExceededException expected) {
            assertEquals("too large", expected.getMessage());
        }
    }

    @Test
    public void cameraProfileScavengerPreservesActiveAndRecentMirrors() throws Exception {
        File filesDirectory = temporaryFolder.newFolder("app-files");
        long nowMillis = 10L * ProfileImporter.CAMERA_PROFILE_MIRROR_GRACE_MILLIS;

        File active = new File(filesDirectory, "camera-profiles-100");
        File stale = new File(filesDirectory, "camera-profiles-200");
        File recent = new File(filesDirectory, "camera-profiles-300");
        File unrelated = new File(filesDirectory, "camera-profiles-manual");
        assertTrue(active.mkdirs());
        assertTrue(stale.mkdirs());
        assertTrue(recent.mkdirs());
        assertTrue(unrelated.mkdirs());
        Files.write(new File(stale, "profile.dcp").toPath(), new byte[] {1});

        long staleTime = nowMillis - ProfileImporter.CAMERA_PROFILE_MIRROR_GRACE_MILLIS - 1L;
        assertTrue(active.setLastModified(staleTime));
        assertTrue(stale.setLastModified(staleTime));
        assertTrue(recent.setLastModified(nowMillis - 1L));
        assertTrue(unrelated.setLastModified(staleTime));

        ProfileImporter.scavengeCameraProfileMirrors(
                filesDirectory, active.getAbsolutePath(), nowMillis);

        assertTrue(active.isDirectory());
        assertFalse(stale.exists());
        assertTrue(recent.isDirectory());
        assertTrue(unrelated.isDirectory());
    }

    @Test
    public void cameraProfileScavengerDoesNotFollowSymlinks() throws Exception {
        File filesDirectory = temporaryFolder.newFolder("symlink-app-files");
        File outside = temporaryFolder.newFolder("outside-profile-target");
        Files.write(new File(outside, "keep.dcp").toPath(), new byte[] {1, 2, 3});

        Path mirrorLink = new File(filesDirectory, "camera-profiles-400").toPath();
        try {
            Files.createSymbolicLink(mirrorLink, outside.toPath());
        } catch (UnsupportedOperationException | IOException | SecurityException error) {
            return;
        }

        File stale = new File(filesDirectory, "camera-profiles-500");
        assertTrue(stale.mkdirs());
        Path nestedLink = new File(stale, "outside-link").toPath();
        Files.createSymbolicLink(nestedLink, outside.toPath());
        long nowMillis = 10L * ProfileImporter.CAMERA_PROFILE_MIRROR_GRACE_MILLIS;
        assertTrue(stale.setLastModified(
                nowMillis - ProfileImporter.CAMERA_PROFILE_MIRROR_GRACE_MILLIS - 1L));

        ProfileImporter.scavengeCameraProfileMirrors(filesDirectory, "", nowMillis);

        assertTrue(Files.isSymbolicLink(mirrorLink));
        assertFalse(stale.exists());
        assertTrue(outside.isDirectory());
        assertTrue(new File(outside, "keep.dcp").isFile());
    }

    @Test
    public void cameraProfileScavengerAbortsForConfiguredPathOutsideOwnedStorage()
            throws Exception {
        File filesDirectory = temporaryFolder.newFolder("validated-app-files");
        File stale = new File(filesDirectory, "camera-profiles-600");
        assertTrue(stale.mkdirs());
        long nowMillis = 10L * ProfileImporter.CAMERA_PROFILE_MIRROR_GRACE_MILLIS;
        assertTrue(stale.setLastModified(
                nowMillis - ProfileImporter.CAMERA_PROFILE_MIRROR_GRACE_MILLIS - 1L));

        File outside = temporaryFolder.newFolder("camera-profiles-700");
        try {
            ProfileImporter.scavengeCameraProfileMirrors(
                    filesDirectory, outside.getAbsolutePath(), nowMillis);
            fail("configured path outside app-owned files should abort scavenging");
        } catch (IllegalArgumentException expected) {
        }

        assertTrue(stale.isDirectory());
        assertTrue(outside.isDirectory());
    }

    @Test
    public void rawLibrarySelectionReadsModifiedTimeOnceAndPreservesStableCutoffTies() {
        CountingFile newest = new CountingFile("newest.dng", 3_000L);
        CountingFile boundaryFirst = new CountingFile("boundary-first.dng", 1_000L);
        CountingFile boundarySecond = new CountingFile("boundary-second.dng", 1_000L);
        CountingFile boundaryThird = new CountingFile("boundary-third.dng", 1_000L);
        CountingFile middle = new CountingFile("middle.dng", 2_000L);
        CountingFile ignored = new CountingFile("ignored.jpg", 9_000L);

        java.util.PriorityQueue<StorageManager.RawLibraryCandidate> retained =
                StorageManager.selectRawLibraryCandidates(
                        new File[] {
                            newest,
                            boundaryFirst,
                            boundarySecond,
                            boundaryThird,
                            middle,
                            ignored
                        },
                        3);

        java.util.HashSet<String> retainedNames = new java.util.HashSet<>();
        for (StorageManager.RawLibraryCandidate candidate : retained) {
            retainedNames.add(candidate.file.getName());
        }
        assertEquals(3, retainedNames.size());
        assertTrue(retainedNames.contains("newest.dng"));
        assertTrue(retainedNames.contains("middle.dng"));
        assertTrue(retainedNames.contains("boundary-first.dng"));
        assertFalse(retainedNames.contains("boundary-second.dng"));
        assertFalse(retainedNames.contains("boundary-third.dng"));

        assertEquals(1, newest.lastModifiedCalls);
        assertEquals(1, boundaryFirst.lastModifiedCalls);
        assertEquals(1, boundarySecond.lastModifiedCalls);
        assertEquals(1, boundaryThird.lastModifiedCalls);
        assertEquals(1, middle.lastModifiedCalls);
        assertEquals(1, ignored.lastModifiedCalls);
    }

    @Test
    public void rawLibraryCutoffStillUsesMillisBeforeSecondLevelOutputOrdering() {
        CountingFile newestWithinSecond = new CountingFile("z-newest.dng", 1_999L);
        CountingFile middleWithinSecond = new CountingFile("a-middle.dng", 1_500L);
        CountingFile oldestWithinSecond = new CountingFile("b-oldest.dng", 1_000L);

        java.util.PriorityQueue<StorageManager.RawLibraryCandidate> retained =
                StorageManager.selectRawLibraryCandidates(
                        new File[] {
                            newestWithinSecond,
                            middleWithinSecond,
                            oldestWithinSecond
                        },
                        2);

        java.util.ArrayList<StorageManager.RawLibraryRecord> records = new java.util.ArrayList<>();
        for (StorageManager.RawLibraryCandidate candidate : retained) {
            String name = candidate.file.getName();
            records.add(new StorageManager.RawLibraryRecord(
                    "file:///" + name, name, "/" + name, 1, candidate.modifiedMillis / 1000));
        }
        records.sort(StorageManager.RAW_LIBRARY_OUTPUT_ORDER);

        assertEquals(2, records.size());
        assertEquals("file:///a-middle.dng", records.get(0).uri);
        assertEquals("file:///z-newest.dng", records.get(1).uri);
    }

    @Test
    public void rawLibraryOutputOrderRemainsModifiedSecondsThenUri() {
        java.util.ArrayList<StorageManager.RawLibraryRecord> records = new java.util.ArrayList<>();
        records.add(new StorageManager.RawLibraryRecord("file:///c.dng", "c.dng", "/c.dng", 1, 10));
        records.add(new StorageManager.RawLibraryRecord("file:///a.dng", "a.dng", "/a.dng", 1, 10));
        records.add(new StorageManager.RawLibraryRecord("file:///b.dng", "b.dng", "/b.dng", 1, 11));

        records.sort(StorageManager.RAW_LIBRARY_OUTPUT_ORDER);

        assertEquals("file:///b.dng", records.get(0).uri);
        assertEquals("file:///a.dng", records.get(1).uri);
        assertEquals("file:///c.dng", records.get(2).uri);
    }

    private static void writeSparseFile(File file, long bytes, long modified) throws Exception {
        try (java.io.RandomAccessFile output = new java.io.RandomAccessFile(file, "rw")) {
            output.setLength(bytes);
        }
        assertTrue(file.setLastModified(modified));
    }

    private static long directoryBytes(File directory) {
        long bytes = 0L;
        File[] entries = directory.listFiles();
        assertTrue(entries != null);
        for (File entry : entries) {
            bytes += Math.max(0L, entry.length());
        }
        return bytes;
    }

    private static final class CountingFile extends File {
        private final long modifiedMillis;
        int lastModifiedCalls;

        CountingFile(String path, long modifiedMillis) {
            super(path);
            this.modifiedMillis = modifiedMillis;
        }

        @Override
        public boolean isFile() {
            return true;
        }

        @Override
        public long lastModified() {
            lastModifiedCalls++;
            return modifiedMillis;
        }
    }

    private static final class ZeroProgressInputStream extends InputStream {
        private final ByteArrayInputStream delegate;
        private boolean returnedZero;

        ZeroProgressInputStream(byte[] input) {
            delegate = new ByteArrayInputStream(input);
        }

        @Override
        public int read(byte[] buffer, int offset, int length) throws IOException {
            if (!returnedZero) {
                returnedZero = true;
                return 0;
            }
            return delegate.read(buffer, offset, length);
        }

        @Override
        public int read() {
            return delegate.read();
        }
    }
}
