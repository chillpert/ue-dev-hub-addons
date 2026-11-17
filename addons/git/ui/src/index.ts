// The Git addon's page, as the hub loads it.
import type { Activate } from "ue-dev-hub-addon-api";
import { setHub } from "./api";
import { enterGit, gitScreen, listenToLog, resetGit } from "./git";
import "./style.css";

const activate: Activate = (hub) => {
  setHub(hub);
  listenToLog();
  return { pages: { git: { render: gitScreen, enter: enterGit, reset: resetGit } } };
};

export default activate;
