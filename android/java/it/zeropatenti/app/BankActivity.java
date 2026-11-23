package it.zeropatenti.app;

import android.app.NativeActivity;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.provider.DocumentsContract;
import android.provider.DocumentsContract.Document;
import android.widget.Toast;
import java.io.File;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.util.HashSet;
import java.util.Set;
import org.json.JSONArray;

/** Reads the bank and its figures from the directory selected in Android's file picker. */
public class BankActivity extends NativeActivity {
    static { System.loadLibrary("zeropatenti"); }
    private static final int CHOOSE_BANK = 107;
    private native boolean bankReady(String file);
    private native String bankFigures(String file);
    private static boolean initialized;

    // Only this process's active workers use these temporary copies.
    private synchronized static File inbox(File cacheDir) throws IOException {
        File parent = new File(cacheDir, "bank-picker");
        Files.createDirectories(parent.toPath());
        if (!initialized) {
            File[] stale = parent.listFiles();
            if (stale == null) throw new IOException("Cartella privata illeggibile");
            for (File dir : stale) {
                if (dir.getName().startsWith("listato-")) discard(dir);
            }
            initialized = true;
        }
        return parent;
    }

    private static void discard(File file) throws IOException {
        if (Files.isDirectory(file.toPath(), java.nio.file.LinkOption.NOFOLLOW_LINKS)) {
            File[] children = file.listFiles();
            if (children == null) throw new IOException("Cartella privata illeggibile");
            for (File child : children) discard(child);
        }
        Files.deleteIfExists(file.toPath());
    }

    @Override
    protected void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        try { inbox(getCacheDir()); }
        catch (IOException e) { android.util.Log.w("ZeroPatenti", "Pulizia importazione rinviata"); }
    }

    public void chooseBank() {
        runOnUiThread(() -> startActivityForResult(new Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), CHOOSE_BANK));
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != CHOOSE_BANK || result != RESULT_OK || data == null || data.getData() == null) return;
        Uri tree = data.getData();
        new Thread(() -> {
            File dir = null;
            try {
                dir = Files.createTempDirectory(inbox(getCacheDir()).toPath(), "listato-").toFile();
                String treeId = DocumentsContract.getTreeDocumentId(tree);
                File bank = null;
                String figuresId = null;
                try (Cursor cursor = children(tree, treeId)) {
                    while (cursor.moveToNext()) {
                        String id = cursor.getString(0), name = cursor.getString(1);
                        if ("manifest.json".equals(name) || "manifest.sig".equals(name)) {
                            if (Document.MIME_TYPE_DIR.equals(cursor.getString(2))) throw new IOException("Manifest o firma non validi");
                            copy(tree, id, new File(dir, name));
                        } else if (Document.MIME_TYPE_DIR.equals(cursor.getString(2))) {
                            if ("figures".equals(name)) figuresId = id;
                        } else if ("questions.json".equals(name) || "questions.csv".equals(name) || "AB2020.json".equals(name)) {
                            if (bank != null) throw new IOException("Seleziona una cartella con un solo listato");
                            bank = new File(dir, name);
                            copy(tree, id, bank);
                        }
                    }
                }
                if (bank == null) throw new IOException("La cartella deve contenere questions.json, questions.csv o AB2020.json");
                if (figuresId != null) {
                    String references = bankFigures(bank.getAbsolutePath());
                    if (references == null) throw new IOException("Listato illeggibile");
                    JSONArray list = new JSONArray(references);
                    Set<String> figures = new HashSet<>();
                    for (int i = 0; i < list.length(); i++) figures.add(list.getString(i));
                    try (Cursor cursor = children(tree, figuresId)) {
                        while (cursor.moveToNext()) {
                            String name = cursor.getString(1);
                            if (Document.MIME_TYPE_DIR.equals(cursor.getString(2)) || name == null
                                    || !name.equals(new File(name).getName()) || !name.matches("(?i).+\\.(png|jpe?g|webp)")
                                    || !figures.contains("figures/" + name)) continue;
                            File target = new File(dir, "figures");
                            Files.createDirectories(target.toPath());
                            copy(tree, cursor.getString(0), new File(target, name));
                        }
                    }
                }
                if (!bankReady(bank.getAbsolutePath())) throw new IOException("Importazione interrotta");
                dir = null; // The native event loop now owns the temporary copy.
            } catch (Exception e) {
                android.util.Log.w("ZeroPatenti", "Importazione cartella interrotta", e);
                runOnUiThread(() -> Toast.makeText(this, e.getLocalizedMessage(), Toast.LENGTH_LONG).show());
            } finally {
                if (dir != null) {
                    try { discard(dir); }
                    catch (IOException e) { android.util.Log.w("ZeroPatenti", "Pulizia importazione rinviata"); }
                }
            }
        }, "Importazione listato").start();
    }

    private Cursor children(Uri tree, String parent) throws IOException {
        Uri uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, parent);
        String[] columns = {Document.COLUMN_DOCUMENT_ID, Document.COLUMN_DISPLAY_NAME, Document.COLUMN_MIME_TYPE};
        Cursor cursor = getContentResolver().query(uri, columns, null, null, null);
        if (cursor == null) throw new IOException("Impossibile leggere la cartella");
        return cursor;
    }

    private void copy(Uri tree, String id, File file) throws IOException {
        Uri document = DocumentsContract.buildDocumentUriUsingTree(tree, id);
        try (InputStream in = getContentResolver().openInputStream(document)) {
            if (in == null) throw new IOException("File illeggibile");
            Files.copy(in, file.toPath());
        }
    }
}
