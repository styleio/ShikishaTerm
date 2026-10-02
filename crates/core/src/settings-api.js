// Authentication and JSON encoding belong to this request boundary. Raw bodies,
// response streams and text downloads keep their own explicit options.
function settingsFetch(path, {json, headers, ...options} = {}) {
  const request = {...options, headers:{"X-Token":TOKEN, ...headers}};
  if (json !== undefined) {
    request.headers["Content-Type"] = "application/json";
    request.body = JSON.stringify(json);
  }
  return fetch(path, request);
}
async function settingsApi(path, body, method = body === undefined ? "GET" : "POST") {
  const response = await settingsFetch(path, {method, json:body});
  const answer = await response.json();
  if (!response.ok) throw new Error(answer.error || `${response.status} ${response.statusText}`);
  return answer;
}
// Callers that present an operation-specific error opt into a result object.
const postJson = (path, body) => settingsApi(path, body, "POST").catch(() => ({ok:false}));
