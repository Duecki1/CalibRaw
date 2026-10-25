package de.duecki.calibraw;

import android.content.Context;
import android.os.SystemClock;
import android.text.InputType;
import android.view.KeyCharacterMap;
import android.view.KeyEvent;
import android.view.View;
import android.view.inputmethod.BaseInputConnection;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;

/**
 * Takes the soft keyboard's focus while the UI edits a number, so the keyboard shows its number
 * layout. NativeActivity's own view offers no editor information, which always gives the text
 * layout. The view is never drawn: NativeActivity owns the window surface.
 *
 * <p>Typed characters reach the native input queue as key events, the same way the text layout
 * delivers them to NativeActivity's view, so egui sees no difference.
 */
final class NumericInputView extends View {
    NumericInputView(Context context) {
        super(context);
        setFocusable(true);
        setFocusableInTouchMode(true);
    }

    @Override
    public boolean onCheckIsTextEditor() {
        return true;
    }

    @Override
    public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
        outAttrs.inputType = InputType.TYPE_CLASS_NUMBER
                | InputType.TYPE_NUMBER_FLAG_DECIMAL
                | InputType.TYPE_NUMBER_FLAG_SIGNED;
        outAttrs.imeOptions = EditorInfo.IME_ACTION_DONE
                | EditorInfo.IME_FLAG_NO_EXTRACT_UI
                | EditorInfo.IME_FLAG_NO_FULLSCREEN;
        return new KeyEventConnection(this);
    }

    /**
     * Turns committed text and deletions into key events. The editor action (Done) becomes Enter
     * through {@link BaseInputConnection#performEditorAction}.
     */
    private static final class KeyEventConnection extends BaseInputConnection {
        private final KeyCharacterMap keys = KeyCharacterMap.load(KeyCharacterMap.VIRTUAL_KEYBOARD);

        KeyEventConnection(View view) {
            super(view, false);
        }

        @Override
        public boolean commitText(CharSequence text, int newCursorPosition) {
            // Characters without a key, such as a Unicode minus, cannot reach the native side.
            KeyEvent[] events = keys.getEvents(text.toString().toCharArray());
            if (events != null) {
                for (KeyEvent event : events) {
                    sendKeyEvent(event);
                }
            }
            return true;
        }

        @Override
        public boolean deleteSurroundingText(int beforeLength, int afterLength) {
            for (int i = 0; i < beforeLength; i++) {
                sendKey(KeyEvent.KEYCODE_DEL);
            }
            for (int i = 0; i < afterLength; i++) {
                sendKey(KeyEvent.KEYCODE_FORWARD_DEL);
            }
            return true;
        }

        private void sendKey(int keyCode) {
            long now = SystemClock.uptimeMillis();
            int flags = KeyEvent.FLAG_SOFT_KEYBOARD | KeyEvent.FLAG_KEEP_TOUCH_MODE;
            for (int action : new int[] {KeyEvent.ACTION_DOWN, KeyEvent.ACTION_UP}) {
                sendKeyEvent(new KeyEvent(now, now, action, keyCode, 0, 0,
                        KeyCharacterMap.VIRTUAL_KEYBOARD, 0, flags));
            }
        }
    }
}
