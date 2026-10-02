package de.duecki.calibraw;

import static org.junit.Assert.assertArrayEquals;

import java.nio.ByteBuffer;
import org.junit.Test;

public final class ReplayVideoEncoderTest {
    @Test
    public void planarInputRespectsBufferOffsetAndRowPadding() {
        ByteBuffer buffer = ByteBuffer.allocate(9);
        buffer.position(1);
        ReplayVideoEncoder.copyPlane(new byte[]{99, 1, 2, 3, 4}, 1, 2, 2, buffer, 4, 1);
        assertArrayEquals(new byte[]{0, 1, 2, 0, 0, 3, 4, 0, 0}, buffer.array());
    }

    @Test
    public void interleavedChromaPreservesTheNeighboringPlaneAndPadding() {
        ByteBuffer shared = ByteBuffer.allocate(12);
        ByteBuffer u = shared.duplicate();
        ByteBuffer v = shared.duplicate();
        v.position(1);
        ReplayVideoEncoder.copyPlane(new byte[]{1, 2, 3, 4}, 0, 2, 2, u, 6, 2);
        ReplayVideoEncoder.copyPlane(new byte[]{5, 6, 7, 8}, 0, 2, 2, v, 6, 2);
        assertArrayEquals(new byte[]{1, 5, 2, 6, 0, 0, 3, 7, 4, 8, 0, 0}, shared.array());
    }
}
