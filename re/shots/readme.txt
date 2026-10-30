# Views for the README gallery: name map x y z pitch yaw roll (feet origin,
# as setpos) [console commands]. Capture with
#   cargo run --release -p portal2 -- --shots re/shots/readme.txt --shots-out re/out/readme --shot-settle-time 4 --shot-map-settle 12
portal_pair sp_a1_intro2 -319.8 60.0 -8.0 12.0 270.0 0 ent_fire portal_red_1 SetActivatedState 1; ent_fire portal_blue_1 SetActivatedState 1
portal_blue sp_a1_intro2 -576.0 96.0 0.0 26.0 180.0 0 ent_fire portal_red_1 SetActivatedState 1; ent_fire portal_blue_1 SetActivatedState 1
overgrown_drips sp_a1_intro5 -104.0 -300.0 236.0 0.0 -90.0 0
goo_pit sp_a2_laser_over_goo 3360.0 -1250.0 150.0 60.0 -90.0 0
laser_glow sp_a2_fizzler_intro 440.0 -870.0 0.0 13.5 -25.8 0
fizzler_field sp_a2_fizzler_intro 128.0 -250.0 0.0 0.0 -90.0 0
turret_camera sp_a2_turret_intro 1032.0 -296.0 128.0 -18.4 0.0 0
gel_walls sp_a3_jump_intro -384 1250 1250 5 270 0
industrial_beams sp_a3_jump_intro -1363.0 1350.0 950.0 -15.0 90.0 0
paint_stream sp_a3_jump_intro -864 1250 950 20 270 0 ent_fire paint_sprayer_jump_2 start
underground_vista sp_a3_01 2000.0 4000.0 3000.0 0.0 0.0 0
underground_lights sp_a3_01 -400.0 2583.0 1340.0 0.0 0.0 0
relaxation_sky sp_a1_intro3 -1376.0 3392.0 100.0 -80.0 90.0 0
flooded_room sp_a1_intro3 140.0 1100.0 -322.0 35.0 90.0 0
