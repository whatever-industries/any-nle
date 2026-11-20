// Adobe Premiere's documented UXP APIs. File exchange keeps the helper local.
const ppro = require("premierepro");
const fs = require("uxp").storage.localFileSystem;

const scratchName = /^Any NLE Import Worker [^/\\]+\.prproj$/;
const guid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function absolute(path) {
  return typeof path === "string" && /^(?:\/|[A-Za-z]:[\\/]|\\\\)/.test(path)
    && !/[\0\r\n]/.test(path);
}

function validate(request) {
  if (request.schema !== "anynle.premiere-import/1"
      || !guid.test(request.request_id) || !guid.test(request.scratch_guid)
      || !absolute(request.scratch_path)
      || !scratchName.test(request.scratch_path.split(/[\\/]/).pop())
      || !Array.isArray(request.originals) || !request.originals.length
      || request.originals.length > 64
      || !request.originals.every((path) => absolute(path))
      || new Set(request.originals).size !== request.originals.length) {
    throw new Error("Invalid request. Supply a distinct set of absolute originals and the scratch project identity.");
  }
}

async function matchedScratch(request) {
  const project = await ppro.Project.getActiveProject();
  if (!project || project.path !== request.scratch_path
      || project.guid.toString().toLowerCase() !== request.scratch_guid.toLowerCase()) {
    throw new Error("Open the exact disposable scratch project named in the request.");
  }
  return project;
}

async function inspect(project, originals) {
  const items = await (await project.getRootItem()).getItems();
  const observed = new Map();
  for (const item of items) {
    const clip = ppro.ClipProjectItem.cast(item);
    if (!clip) throw new Error("Unexpected item in the scratch root.");
    const path = await clip.getMediaFilePath();
    if (!originals.has(path) || observed.has(path)) {
      throw new Error(`Unexpected or duplicate imported path: ${path}`);
    }
    const offline = await clip.isOffline();
    const media = await clip.getMedia();
    observed.set(path, {
      path, online: !offline,
      project_item_id: String(await item.getId()),
      start_ticks: media ? String(media.getStart().ticks) : null,
      duration_ticks: media ? String(media.getDuration().ticks) : null
    });
  }
  return observed;
}

async function importOriginals(request, outputFolder) {
  validate(request);
  const project = await matchedScratch(request);
  const root = await project.getRootItem();
  if ((await root.getItems()).length || (await project.getSequences()).length) {
    throw new Error("The disposable scratch project must be empty.");
  }
  const resultsName = `Any NLE Import ${request.request_id.toLowerCase()}`;
  if ((await outputFolder.getEntries()).some((entry) => entry.name.toLowerCase() === resultsName.toLowerCase())) {
    throw new Error("This import request already has a results folder.");
  }
  const results = await outputFolder.createFolder(resultsName);
  const receipt = { schema: "anynle.premiere-import-result/1", request_id: request.request_id, files: [] };
  try {
    const settings = await ppro.ProjectSettings.getIngestSettings(project);
    if (await settings.getIsIngestEnabled()) {
      if (!await settings.setIngestEnabled(false)) throw new Error("Could not turn off scratch-project ingest.");
      let committed = false;
      project.lockedAccess(() => {
        committed = project.executeTransaction((transaction) => {
          transaction.addAction(ppro.ProjectSettings.createSetIngestSettingsAction(project, settings));
        }, "Turn off ingest for the Any NLE import trial");
      });
      if (!committed) throw new Error("Could not apply scratch-project ingest settings.");
    }
    await matchedScratch(request);
    if (await (await ppro.ProjectSettings.getIngestSettings(project)).getIsIngestEnabled()) {
      throw new Error("Scratch-project ingest is still enabled.");
    }
    const originals = new Set(request.originals);
    const imported = await project.importFiles(request.originals, true, null, false);
    const deadline = Date.now() + 30_000;
    do {
      await matchedScratch(request);
      const observed = await inspect(project, originals);
      receipt.files = request.originals.map((path) => observed.get(path) ?? { path, online: false });
      if (!imported || receipt.files.every((file) => file.online)) break;
      await new Promise((resolve) => setTimeout(resolve, 250));
    } while (Date.now() < deadline);
    if (!imported || !receipt.files.every((file) => file.online)) {
      throw new Error("Premiere did not import every requested original online.");
    }
    const separator = results.nativePath.includes("\\") ? "\\" : "/";
    const donorPath = results.nativePath + separator + "Donor.prproj";
    if ((await results.getEntries()).length) throw new Error("The new results folder is occupied.");
    await matchedScratch(request);
    if (!await project.saveAs(donorPath)) throw new Error("Premiere could not save the donor.");
    const donor = await results.getEntry("Donor.prproj");
    if (!donor.isFile || donor.nativePath !== project.path) throw new Error("Saved donor path differs.");
    receipt.donor = donor.nativePath;
  } catch (error) {
    receipt.error = String(error.message ?? error);
    throw error;
  } finally {
    const file = await results.createFile("capture.json", { overwrite: false });
    await file.write(JSON.stringify(receipt, null, 2));
  }
  return receipt;
}

module.exports = { ppro, fs, guid, scratchName, importOriginals, validate };
