import json, urllib.request, urllib.parse

BASE = "http://mattermost:8065/api/v4"

# Login as admin
login_data = json.dumps({
    "login_id": "hermes@local.mattermost",
    "password": "HermesAdmin!2026Secure",
}).encode()

req = urllib.request.Request(
    f"{BASE}/users/login",
    data=login_data,
    headers={"Content-Type": "application/json"},
)
resp = urllib.request.urlopen(req)
token = resp.headers.get("Token")
print("Got admin token:", token[:10] + "...")

# GET current config
headers = {"Authorization": f"Bearer {token}"}
req = urllib.request.Request(f"{BASE}/config", headers=headers)
resp = urllib.request.urlopen(req)
cfg = json.loads(resp.read())

print("Current SiteURL:", cfg.get("ServiceSettings", {}).get("SiteURL", "NOT SET"))

# Modify SiteURL
cfg["ServiceSettings"]["SiteURL"] = "https://hermes-app-5.nexuslbs.org"

# PUT updated config
data = json.dumps(cfg).encode()
req = urllib.request.Request(
    f"{BASE}/config", data=data, headers=headers, method="PUT"
)
resp = urllib.request.urlopen(req)
result = json.loads(resp.read())

print("New SiteURL:", result.get("ServiceSettings", {}).get("SiteURL", "NOT SET"))
