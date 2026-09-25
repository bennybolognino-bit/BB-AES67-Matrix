use mdns_sd::{ServiceDaemon, ServiceEvent};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::HashMap, net::IpAddr, time::Duration};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NmosRegistry {
    name: String,
    hostname: String,
    url: String,
    api_version: String,
    priority: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NmosNode {
    id: String,
    label: String,
    hostname: String,
    description: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NmosDevice {
    id: String,
    node_id: String,
    label: String,
    description: String,
    connection_api: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NmosSender {
    id: String,
    device_id: String,
    flow_id: String,
    label: String,
    description: String,
    transport: String,
    manifest_href: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NmosReceiver {
    id: String,
    device_id: String,
    label: String,
    description: String,
    transport: String,
    format: String,
    subscribed_sender_id: String,
    connection_api: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NmosResources {
    registry_url: String,
    nodes: Vec<NmosNode>,
    devices: Vec<NmosDevice>,
    senders: Vec<NmosSender>,
    receivers: Vec<NmosReceiver>,
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|error| error.to_string())
}

fn normalize_query_url(input: &str, client: &Client) -> Result<String, String> {
    let input = input.trim().trim_end_matches('/');

    if input.contains("/x-nmos/query/v1.") {
        return Ok(input.to_string());
    }

    let root = input
        .trim_end_matches("/x-nmos/query")
        .trim_end_matches('/');

    for version in ["v1.3", "v1.2", "v1.1", "v1.0"] {
        let candidate = format!("{root}/x-nmos/query/{version}");

        if client
            .get(format!("{candidate}/nodes"))
            .send()
            .map(|response| response.status().is_success())
            .unwrap_or(false)
        {
            return Ok(candidate);
        }
    }

    Err("Nessuna Query API NMOS compatibile trovata".into())
}

fn query_list(client: &Client, base: &str, resource: &str) -> Result<Vec<Value>, String> {
    let response = client
        .get(format!("{base}/{resource}"))
        .send()
        .map_err(|error| error.to_string())?;

    if !response.status().is_success() {
        return Err(format!("NMOS {resource}: HTTP {}", response.status()));
    }

    response
        .json::<Vec<Value>>()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn discover_nmos_registries() -> Result<Vec<NmosRegistry>, String> {
    let daemon = ServiceDaemon::new().map_err(|error| error.to_string())?;
    let service_type = "_nmos-query._tcp.local.";
    let receiver = daemon
        .browse(service_type)
        .map_err(|error| error.to_string())?;

    let mut registries = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);

    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());

        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let protocol = info.get_property_val_str("api_proto").unwrap_or("http");

                let versions = info.get_property_val_str("api_ver").unwrap_or("v1.2");

                let version = versions.split(',').map(str::trim).max().unwrap_or("v1.2");

                let priority = info
                    .get_property_val_str("pri")
                    .unwrap_or_default()
                    .to_string();

                for address in info.get_addresses() {
                    if let IpAddr::V4(address) = address {
                        let url = format!(
                            "{protocol}://{}:{}/x-nmos/query/{version}",
                            address,
                            info.get_port()
                        );

                        if !registries.iter().any(|item: &NmosRegistry| item.url == url) {
                            registries.push(NmosRegistry {
                                name: info.get_fullname().to_string(),
                                hostname: info.get_hostname().to_string(),
                                url,
                                api_version: version.to_string(),
                                priority: priority.clone(),
                            });
                        }
                    }
                }
            }

            Ok(_) => {}
            Err(_) => break,
        }
    }

    let _ = daemon.stop_browse(service_type);
    let _ = daemon.shutdown();

    Ok(registries)
}

#[tauri::command]
pub fn query_nmos_registry(url: String) -> Result<NmosResources, String> {
    let client = client()?;
    let base = normalize_query_url(&url, &client)?;

    let node_values = query_list(&client, &base, "nodes")?;
    let device_values = query_list(&client, &base, "devices")?;
    let sender_values = query_list(&client, &base, "senders")?;
    let receiver_values = query_list(&client, &base, "receivers")?;

    let nodes = node_values
        .iter()
        .map(|value| NmosNode {
            id: string(value, "id"),
            label: string(value, "label"),
            hostname: string(value, "hostname"),
            description: string(value, "description"),
        })
        .collect();

    let mut connection_apis = HashMap::new();
    let mut devices = Vec::new();

    for value in &device_values {
        let connection_api = value
            .get("controls")
            .and_then(Value::as_array)
            .and_then(|controls| {
                controls.iter().find(|control| {
                    string(control, "type").contains("sr-ctrl")
                        || string(control, "href").contains("/connection/")
                })
            })
            .map(|control| string(control, "href"))
            .unwrap_or_default();

        let id = string(value, "id");
        connection_apis.insert(id.clone(), connection_api.clone());

        devices.push(NmosDevice {
            id,
            node_id: string(value, "node_id"),
            label: string(value, "label"),
            description: string(value, "description"),
            connection_api,
        });
    }

    let senders = sender_values
        .iter()
        .map(|value| NmosSender {
            id: string(value, "id"),
            device_id: string(value, "device_id"),
            flow_id: string(value, "flow_id"),
            label: string(value, "label"),
            description: string(value, "description"),
            transport: string(value, "transport"),
            manifest_href: string(value, "manifest_href"),
        })
        .collect();

    let receivers = receiver_values
        .iter()
        .map(|value| {
            let device_id = string(value, "device_id");

            let subscribed_sender_id = value
                .get("subscription")
                .and_then(|subscription| subscription.get("sender_id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();

            NmosReceiver {
                id: string(value, "id"),
                device_id: device_id.clone(),
                label: string(value, "label"),
                description: string(value, "description"),
                transport: string(value, "transport"),
                format: string(value, "format"),
                subscribed_sender_id,
                connection_api: connection_apis.get(&device_id).cloned().unwrap_or_default(),
            }
        })
        .collect();

    Ok(NmosResources {
        registry_url: base,
        nodes,
        devices,
        senders,
        receivers,
    })
}

#[tauri::command]
pub fn connect_nmos_receiver(
    connection_api: String,
    receiver_id: String,
    sender_id: Option<String>,
) -> Result<String, String> {
    if connection_api.trim().is_empty() {
        return Err("Il receiver non espone IS-05".into());
    }

    let endpoint = format!(
        "{}/single/receivers/{}/staged",
        connection_api.trim_end_matches('/'),
        receiver_id
    );

    let enabled = sender_id.as_ref().is_some_and(|value| !value.is_empty());

    let body = json!({
        "sender_id": if enabled { sender_id } else { None::<String> },
        "master_enable": enabled,
        "activation": {
            "mode": "activate_immediate",
            "requested_time": Value::Null
        }
    });

    let response = client()?
        .patch(endpoint)
        .json(&body)
        .send()
        .map_err(|error| error.to_string())?;

    let status = response.status();
    let response_body = response.text().unwrap_or_default();

    if !status.is_success() {
        return Err(format!("IS-05 HTTP {status}: {response_body}"));
    }

    Ok(if enabled {
        "Connessione NMOS attivata".into()
    } else {
        "Receiver NMOS scollegato".into()
    })
}
