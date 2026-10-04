package de.duecki.calibraw;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import org.junit.Test;

public class ProfileImporterTest {
    /** A stream of {@code length} zero bytes that allocates nothing. */
    private static InputStream zeros(long length) {
        return new InputStream() {
            private long remaining = length;

            @Override
            public int read() {
                if (remaining <= 0) {
                    return -1;
                }
                remaining--;
                return 0;
            }

            @Override
            public int read(byte[] buffer, int offset, int count) {
                if (remaining <= 0) {
                    return -1;
                }
                int read = (int) Math.min(count, remaining);
                remaining -= read;
                return read;
            }
        };
    }

    @Test
    public void copiesAProfileAndReportsItsSize() throws Exception {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        long copied = ProfileImporter.copyProfile(
                new ByteArrayInputStream(new byte[] {1, 2, 3}), output, 0L);
        assertEquals(3L, copied);
        assertEquals(3, output.size());
    }

    @Test
    public void aProfileMayFillTheFileLimitButNotExceedIt() throws Exception {
        long limit = ProfileImporter.MAX_DCP_FILE_BYTES;
        assertEquals(limit, ProfileImporter.copyProfile(
                zeros(limit), OutputStream.nullOutputStream(), 0L));
        StorageLimitExceededException error = assertThrows(
                StorageLimitExceededException.class,
                () -> ProfileImporter.copyProfile(
                        zeros(limit + 1), OutputStream.nullOutputStream(), 0L));
        assertTrue(error.getMessage(), error.getMessage().startsWith("A DCP exceeds"));
    }

    @Test
    public void theTreeLimitAppliesWhenItIsTighter() throws Exception {
        long alreadyImported = ProfileImporter.MAX_DCP_TREE_BYTES - 3L;
        assertEquals(3L, ProfileImporter.copyProfile(
                zeros(3), OutputStream.nullOutputStream(), alreadyImported));
        StorageLimitExceededException error = assertThrows(
                StorageLimitExceededException.class,
                () -> ProfileImporter.copyProfile(
                        zeros(4), OutputStream.nullOutputStream(), alreadyImported));
        assertTrue(error.getMessage(), error.getMessage().startsWith("The selected profile tree"));
        // An empty profile still fits a full tree.
        assertEquals(0L, ProfileImporter.copyProfile(
                zeros(0), OutputStream.nullOutputStream(), ProfileImporter.MAX_DCP_TREE_BYTES));
    }
}
