package de.duecki.calibraw;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.os.Bundle;
import android.os.Process;

/**
 * Owns the GPU-loss notice after the editor saved and ended its process.
 * This separate process uses Android UI only; its lifecycle never waits for
 * the editor's blocked or lost native renderer.
 */
public final class GpuLossRestartActivity extends Activity {
    /** The process that is ending, which this one ends for certain first. */
    static final String EXTRA_ENDING_PID = "de.duecki.calibraw.ENDING_PID";
    static final String EXTRA_TITLE = "de.duecki.calibraw.GPU_LOSS_TITLE";
    static final String EXTRA_MESSAGE = "de.duecki.calibraw.GPU_LOSS_MESSAGE";
    static final String EXTRA_RESTART_ALLOWED = "de.duecki.calibraw.GPU_LOSS_RESTART_ALLOWED";
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
        if (savedInstanceState == null && endingPid > 0 && endingPid != Process.myPid()) {
            Process.killProcess(endingPid);
        }
        new AlertDialog.Builder(this)
                .setTitle(getIntent().getStringExtra(EXTRA_TITLE))
                .setMessage(getIntent().getStringExtra(EXTRA_MESSAGE))
                .setCancelable(false)
                .setPositiveButton(android.R.string.ok, (dialog, which) -> finishRecovery())
                .show();
    }

    private void finishRecovery() {
        if (!getIntent().getBooleanExtra(EXTRA_RESTART_ALLOWED, false)) {
            finishAndRemoveTask();
            Process.killProcess(Process.myPid());
            return;
        }
        Intent calibRaw = new Intent(this, CalibRawActivity.class)
                .setAction(Intent.ACTION_MAIN)
                .addCategory(Intent.CATEGORY_LAUNCHER)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TASK)
                .putExtra(EXTRA_RESTARTED_AFTER_GPU_LOSS, true);
        startActivity(calibRaw);
        finish();
        Process.killProcess(Process.myPid());
    }
}
