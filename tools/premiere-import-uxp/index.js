const { ppro, fs, scratchName, importOriginals, validate } = require("./capture.js");
let busy = false;

async function withStatus(action) {
  if (busy) return;
  busy = true;
  const status = document.getElementById("status");
  try {
    status.textContent = await action();
  } catch (error) {
    status.textContent = String(error.message ?? error);
  } finally {
    busy = false;
  }
}

document.getElementById("identify").addEventListener("click", () => withStatus(async () => {
  const project = await ppro.Project.getActiveProject();
  if (!project || !scratchName.test(project.path.split(/[\\/]/).pop())) {
    throw new Error("Open a disposable project named Any NLE Import Worker … .prproj.");
  }
  return JSON.stringify({ scratch_path: project.path, scratch_guid: project.guid.toString() }, null, 2);
}));

document.getElementById("import").addEventListener("click", () => withStatus(async () => {
  const file = await fs.getFileForOpening({ types: ["json"] });
  if (!file) return "Request selection cancelled.";
  const metadata = await file.getMetadata();
  if (metadata.size > 65_536) throw new Error("Import request exceeds 64 KiB.");
  const request = JSON.parse(await file.read());
  validate(request);
  const folder = await fs.getFolder();
  if (!folder) return "Results folder selection cancelled.";
  const receipt = await importOriginals(request, folder);
  return `Saved ${receipt.files.length} linked originals to ${receipt.donor}`;
}));
