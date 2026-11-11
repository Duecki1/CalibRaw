package de.duecki.calibraw;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.os.Process;

/**
 * Starts CalibRaw again after its process ended because the GPU device was
 * lost ({@link CalibRawActivity#restartAfterGpuLoss}). It runs in a process of
 * its own (":gpu_restart" in the manifest): an activity started from the
 * ending process could be delivered to it before it ends, and end with it.
 */
public final class GpuLossRestartActivity extends Activity {
    /** The process that is ending, which this one ends for certain first. */
    static final String EXTRA_ENDING_PID = "de.duecki.calibraw.ENDING_PID";
    /**
     * Set on the restarted CalibRawActivity, so that a GPU that fails again
     * right away does not restart CalibRaw in a loop.
     */
    static final String EXTRA_RESTARTED_AFTER_GPU_LOSS =
            "de.duecki.calibraw.RESTARTED_AFTER_GPU_LOSS";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        int endingPid = getIntent().getIntExtra(EXTRA_ENDING_PID, -1);
        if (endingPid > 0 && endingPid != Process.myPid()) {
            Process.killProcess(endingPid);
        }
        Intent calibRaw = new Intent(this, CalibRawActivity.class)
                .setAction(Intent.ACTION_MAIN)
                .addCategory(Intent.CATEGORY_LAUNCHER)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK)
                .putExtra(EXTRA_RESTARTED_AFTER_GPU_LOSS, true);
        startActivity(calibRaw);
        finish();
        Runtime.getRuntime().exit(0);
    }
}
