package de.duecki.calibraw;

import android.media.Image;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaCodecList;
import android.media.MediaFormat;
import android.media.MediaMuxer;
import android.os.SystemClock;

import java.nio.ByteBuffer;

/** Synchronous worker-owned H.264 encoder. Input is planar, limited-range BT.601 YUV420. */
final class ReplayVideoEncoder implements AutoCloseable {
    private static final long TIMEOUT_US = 10_000;
    private static final long STALL_TIMEOUT_MS = 30_000;
    private final int width;
    private final int height;
    private final int fps;
    private final MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
    private MediaCodec codec;
    private MediaMuxer muxer;
    private boolean codecStarted;
    private boolean muxerStarted;
    private boolean outputEnded;
    private int track = -1;
    private long frameCount;

    ReplayVideoEncoder(String path, int width, int height, int fps) throws Exception {
        this.width = width;
        this.height = height;
        this.fps = fps;
        if (width <= 0 || height <= 0 || (width & 1) != 0 || (height & 1) != 0 || fps <= 0) {
            throw new IllegalArgumentException("Replay requires positive even dimensions and FPS");
        }
        try {
            MediaFormat format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height);
            format.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible);
            format.setInteger(MediaFormat.KEY_BIT_RATE, 8_000_000);
            format.setInteger(MediaFormat.KEY_FRAME_RATE, fps);
            format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 1);
            format.setInteger(MediaFormat.KEY_COLOR_STANDARD, MediaFormat.COLOR_STANDARD_BT601_NTSC);
            format.setInteger(MediaFormat.KEY_COLOR_RANGE, MediaFormat.COLOR_RANGE_LIMITED);
            format.setInteger(MediaFormat.KEY_COLOR_TRANSFER, MediaFormat.COLOR_TRANSFER_SDR_VIDEO);
            String name = new MediaCodecList(MediaCodecList.REGULAR_CODECS).findEncoderForFormat(format);
            if (name == null) {
                throw new IllegalStateException("No Android H.264 encoder supports this replay size: " + width + "x" + height);
            }
            codec = MediaCodec.createByCodecName(name);
            codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
            muxer = new MediaMuxer(path, MediaMuxer.OutputFormat.MUXER_OUTPUT_MPEG_4);
            codec.start();
            codecStarted = true;
        } catch (Exception error) {
            close();
            throw error;
        }
    }

    void writeFrame(byte[] yuv) throws Exception {
        if (yuv.length != width * height * 3 / 2) {
            throw new IllegalArgumentException("Invalid replay frame length");
        }
        int index = awaitInput();
        Image image = codec.getInputImage(index);
        if (image == null) {
            throw new IllegalStateException("Android encoder did not provide a YUV input image");
        }
        try {
            Image.Plane[] planes = image.getPlanes();
            copyPlane(yuv, 0, width, height, planes[0]);
            copyPlane(yuv, width * height, width / 2, height / 2, planes[1]);
            copyPlane(yuv, width * height * 5 / 4, width / 2, height / 2, planes[2]);
        } finally {
            image.close();
        }
        codec.queueInputBuffer(index, 0, yuv.length, frameCount * 1_000_000L / fps, 0);
        frameCount++;
        drain(false);
    }

    private static void copyPlane(byte[] data, int offset, int width, int height, Image.Plane plane) {
        copyPlane(data, offset, width, height, plane.getBuffer(), plane.getRowStride(), plane.getPixelStride());
    }

    static void copyPlane(byte[] data, int offset, int width, int height,
            ByteBuffer buffer, int stride, int pixelStride) {
        int base = buffer.position();
        for (int row = 0; row < height; row++) {
            int source = offset + row * width;
            int destination = base + row * stride;
            if (pixelStride == 1) {
                buffer.position(destination);
                buffer.put(data, source, width);
            } else {
                for (int col = 0; col < width; col++) {
                    buffer.put(destination + col * pixelStride, data[source + col]);
                }
            }
        }
    }

    private int awaitInput() throws Exception {
        long deadline = SystemClock.elapsedRealtime() + STALL_TIMEOUT_MS;
        while (true) {
            int index = codec.dequeueInputBuffer(TIMEOUT_US);
            if (index >= 0) {
                return index;
            }
            drain(false);
            if (SystemClock.elapsedRealtime() >= deadline) {
                throw new IllegalStateException("Android replay encoder stalled waiting for input");
            }
        }
    }

    private void drain(boolean ending) throws Exception {
        long deadline = SystemClock.elapsedRealtime() + STALL_TIMEOUT_MS;
        while (!outputEnded) {
            int index = codec.dequeueOutputBuffer(info, ending ? TIMEOUT_US : 0);
            if (index == MediaCodec.INFO_TRY_AGAIN_LATER) {
                if (!ending) {
                    return;
                }
                if (SystemClock.elapsedRealtime() >= deadline) {
                    throw new IllegalStateException("Android replay encoder stalled finalizing video");
                }
            } else if (index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                if (muxerStarted) {
                    throw new IllegalStateException("Replay encoder changed format after starting");
                }
                track = muxer.addTrack(codec.getOutputFormat());
                muxer.start();
                muxerStarted = true;
            } else if (index >= 0) {
                try {
                    if ((info.flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) == 0 && info.size > 0) {
                        if (!muxerStarted) {
                            throw new IllegalStateException("Replay encoder returned video before its format");
                        }
                        ByteBuffer buffer = codec.getOutputBuffer(index);
                        if (buffer == null) {
                            throw new IllegalStateException("Replay encoder returned no output buffer");
                        }
                        buffer.position(info.offset);
                        buffer.limit(info.offset + info.size);
                        muxer.writeSampleData(track, buffer, info);
                    }
                    outputEnded = (info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0;
                } finally {
                    codec.releaseOutputBuffer(index, false);
                }
                deadline = SystemClock.elapsedRealtime() + STALL_TIMEOUT_MS;
            }
        }
    }

    void finish() throws Exception {
        try {
            int index = awaitInput();
            codec.queueInputBuffer(index, 0, 0, frameCount * 1_000_000L / fps,
                    MediaCodec.BUFFER_FLAG_END_OF_STREAM);
            drain(true);
            if (!muxerStarted) {
                throw new IllegalStateException("Android replay encoder produced no video");
            }
            // A failed stop must fail the export, since it writes the MP4 index.
            muxer.stop();
            muxerStarted = false;
        } finally {
            close();
        }
    }

    @Override
    public void close() {
        if (codec != null) {
            try {
                if (codecStarted) codec.stop();
            } catch (RuntimeException ignored) {
                // Release resources even after a codec failure or cancellation.
            } finally {
                try { codec.release(); } catch (RuntimeException ignored) { }
                codec = null;
            }
        }
        if (muxer != null) {
            try {
                if (muxerStarted) muxer.stop();
            } catch (RuntimeException ignored) {
                // An unfinished export is discarded by the Rust worker.
            } finally {
                try { muxer.release(); } catch (RuntimeException ignored) { }
                muxer = null;
            }
        }
    }
}
