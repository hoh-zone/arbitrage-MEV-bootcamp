//! Navi is used exclusively for flashloan.

use sui_types::{base_types::ObjectID, dynamic_field::derive_dynamic_field_id, TypeTag};

pub fn navi_related_object_ids() -> Vec<String> {
    let mut res = vec![
        "0xd899cf7d2b5db716bd2cf55599fb0d5ee38a3061e7b6bb6eebf73fa5bc4c81ca", // NaviProtocol 7
        "0x06007a2d0ddd3ef4844c6d19c83f71475d6d3ac2d139188d6b62c052e6965edd", // NaviProtocol 9
        "0x834a86970ae93a73faf4fff16ae40bdb72b91c47be585fff19a2af60a19ddca3", // NaviProtocol 20
        "0x1951eff08b3fd5bd134df6787ec9ec533c682d74277b824dbd53e440926901ad", // NaviProtocol 21
        "0xc2d49bf5e75d2258ee5563efa527feb6155de7ac6f6bf025a23ee88cd12d5a83", // NaviProtocol 22
        "0x96df0fce3c471489f4debaaa762cf960b3d97820bd1f3f025ff8190730e958c5", // NaviPool
        "0x3672b2bf471a60c30a03325f104f92fb195c9d337ba58072dce764fe2aa5e2dc", // NaviConfig
        "0x3dea04b6029fa398581cfac0f70f6fcf6ff4ddd9e9852b1a7374395196394de1", // NaviAsset
        "0x48e3820fe5cc11bd6acf0115b496070c2e9d2077938a7818a06c23d0bb33ad69", // NaviAssetConfig
        "0xbb4e2f4b6205c2e2a2db47aeb4f830796ec7c005f88537ee775986639bc442fe", // NaviStorage
        "0x9a91a751ff83ef1eb940066a60900d479cbd39c6eaccdd203632c97dedd10ce9", // NaviReserveData
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>();

    let storage_children_ids = {
        let mut res = vec![];

        let parent_id =
            ObjectID::from_hex_literal("0xe6d4c6610b86ce7735ea754596d71d72d10c7980b5052fc3c8cdf8d09fea9b4b").unwrap();

        let key_tag = TypeTag::U8;

        for i in 0..20u8 {
            let key_bytes = bcs::to_bytes(&i).unwrap();
            let child_id = derive_dynamic_field_id(parent_id, &key_tag, &key_bytes).unwrap();
            res.push(child_id.to_string());
        }

        res
    };

    res.extend(storage_children_ids);

    res
}
