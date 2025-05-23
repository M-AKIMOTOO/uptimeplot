#!/usr/bin/env python3
# AKIMOTO
# 2023-07-08


import os
import subprocess
import platform
import numpy as np
import matplotlib.pyplot as plt
from astropy import units as u
from astropy.coordinates import SkyCoord, EarthLocation, AltAz, Angle, get_sun
from astropy.time import Time

plt.rcParams["xtick.direction"]     = "in"       
plt.rcParams["ytick.direction"]     = "in"       
plt.rcParams["xtick.minor.visible"] = True       
plt.rcParams["ytick.minor.visible"] = True
plt.rcParams["xtick.top"]           = True
plt.rcParams["xtick.bottom"]        = True
plt.rcParams["ytick.left"]          = True
plt.rcParams["ytick.right"]         = True 
plt.rcParams["xtick.major.size"]    = 5          
plt.rcParams["ytick.major.size"]    = 5          
plt.rcParams["xtick.minor.size"]    = 3          
plt.rcParams["ytick.minor.size"]    = 3          
plt.rcParams["axes.grid"]           = True
plt.rcParams["grid.color"]          = "lightgray"
plt.rcParams["axes.labelsize"]      = 12
plt.rcParams["font.size"]           = 12

def RaDec2AltAz(object_ra: float, object_dec: float, observation_time: float, latitude: float, longitude: float, height: float) -> float :
    
    # AZ-EL
    location_geocentrice = EarthLocation.from_geocentric(latitude, longitude, height, unit=u.m)
    location_geodetic    = EarthLocation.to_geodetic(location_geocentrice)
    location_lon_lat     = EarthLocation(lon=location_geodetic.lon, lat=location_geodetic.lat, height=location_geodetic.height)
    
    one_day_minutes      = np.linspace(0, 24, 24*10) * u.hour
    obstime              = Time(observation_time, scale="utc") + one_day_minutes
    object_ra_dec        = SkyCoord(ra=object_ra * u.deg, dec=object_dec * u.deg)
    AltAz_coord          = AltAz(location=location_lon_lat, obstime=obstime)
    object_altaz         = object_ra_dec.transform_to(AltAz_coord)
    
    # sun
    sun = get_sun(obstime).transform_to(AltAz_coord)
    
    # LST
    object_ra_dec        = SkyCoord(ra=object_ra * u.deg, dec=object_dec * u.deg)
    obstime_lst          = Time(observation_time, scale="utc" ,location=location_lon_lat) + one_day_minutes
    AltAz_coord          = AltAz(location=location_lon_lat, obstime=obstime_lst)
    lst_altaz            = object_ra_dec.transform_to(AltAz_coord)
    lst                  = obstime_lst.sidereal_time('apparent')
    hourangle            = obstime_lst.sidereal_time('apparent', 'greenwich')
    
    return one_day_minutes, object_altaz.az.deg, object_altaz.alt.deg, lst.hour, lst_altaz.az.deg, lst_altaz.alt.deg, sun.az, sun.alt

config = "/home/akimoto/program/python/uptimeplot.config"

# For checking uptimeplot-config file
print(f"Please edit \"{config}\"")
pc_platform = platform.system()
if pc_platform == "Windows" :
    subprocess.call(["notepad", f"{config}"])
elif pc_platform == "Darwin" :
    subprocess.call(["open", f"{config}"])
elif pc_platform == "Linux" :
    subprocess.call(["gedit", f"{config}", "--new-window"])
    
if pc_platform == "Windows" : 
    ans = "y"
else :
    ans = input("If you have edited the config-file, Please Enter-key or [y/n]: ")
if ans in ["", "y"] :
    pass
else :
    print("Please start over!!")
    exit(1)

uptimeplot_config = open(config, "r").readlines()
# flag
antenna_flag_st, antenna_flag_ed = False, False
date_flag_st   , date_flag_ed    = False, False
target_flag_st , target_flag_ed  = False, False
antenn_dict = {}
date_list   = []
target_dict = {}
for confing_read in uptimeplot_config :
    
    try :
        confing_read = confing_read[:-1].split()
        
        if   confing_read[0] == "ANTENNA-FLAG-ST" : antenna_flag_st = True; continue
        elif confing_read[0] == "ANTENNA-FLAG-ED" : antenna_flag_ed = True; continue
        elif confing_read[0] == "DATE-FLAG-ST"    : date_flag_st    = True; continue
        elif confing_read[0] == "DATE-FLAG-ED"    : date_flag_ed    = True; continue
        elif confing_read[0] == "TARGET-FLAG-ST"  : target_flag_st  = True; continue
        elif confing_read[0] == "TARGET-FLAG-ED"  : target_flag_ed  = True; continue
        else : pass

        confing_read_split = " ".join(confing_read).split()

        # empty line
        if len(confing_read_split) == 0 :
            continue
        
        # Skipping a commentout-line
        if "#" in confing_read_split[0] :
            continue
        
        
        if antenna_flag_st == True and antenna_flag_ed == False :
            antenn_dict[confing_read_split[0]] = " ".join(confing_read_split[1:])
        elif date_flag_st == True and date_flag_ed == False :
            date_list.append(confing_read_split[0])
        elif target_flag_st == True and target_flag_ed == False :
            target_dict[confing_read_split[0]] = " ".join(confing_read_split[2:8])
        else :
            pass
        #
    except IndexError :
        continue

if len(antenn_dict) == 0 :
    print("Please specify antenna using your observation!!")
    exit(1)
if len(date_list) == 0 :
    print("Please specify your observation date!!")
    exit(1)
if len(target_dict) == 0 :
    print("Please specify targets whitch you observe!!")
    exit(1)
        
    
    
antenna_list = list(antenn_dict.keys())
target_list  = list(target_dict.keys())

print()
print("ANTENNA: %s" % ", ".join(antenna_list))
print("TARGET: %s" % ", ".join(target_list))
print()

print("RUN!!")
target_num = 0
for antenna in antenna_list :
    
    print(f"   {antenna}")
    
    antenna_position_x, antenna_position_y, antenna_position_z = antenn_dict[antenna].split()
    
    for date in date_list :
        
        save_pass1 = "/home/akimoto/uptimeplot/uptimeplot_output/"
        save_pass2 = f"{save_pass1}/{date}/{antenna}"
        os.makedirs(save_pass2, exist_ok=True)
        
        fig_azel , axs_azel  = plt.subplots(2, 1, figsize=(12,9), sharex=True, tight_layout=True)
        fig_polar, axs_polar = plt.subplots(1, 1, figsize=(8, 9)             , tight_layout=True, subplot_kw={'projection': 'polar'})
        fig_lst  , axs_lst   = plt.subplots(2, 1, figsize=(12,9), sharex=True, tight_layout=True)
        
        for t, target in enumerate(target_list) :
            
            target_num += 1
            if target_num == 19 :
                target_num = 0
                break
            
            cm = plt.colormaps.get_cmap("tab20")
            rgb = cm.colors[t]
            
            target_ra_h, target_ra_m, target_ra_s, target_dec_d, target_dec_m, target_dec_s = target_dict[target].split()
            target_ra  = Angle("%dh%dm%fs" % (float(target_ra_h) , float(target_ra_m) , float(target_ra_s)) , unit='hourangle').degree
            target_dec = Angle("%+dd%dm%fs" % (float(target_dec_d), float(target_dec_m), float(target_dec_s)), unit=u.deg).degree
        
            target_datetime, target_az, target_el, target_lst, target_lst_az, target_lst_el, sun_az, sun_el = RaDec2AltAz(float(target_ra), float(target_dec), date, float(antenna_position_x), float(antenna_position_y), float(antenna_position_z))


            axs_azel[0].plot(target_datetime, target_az, label=f"{target}", color=rgb) # az
            axs_azel[1].plot(target_datetime, target_el, label=f"{target}", color=rgb) # el
            
            axs_polar.plot(target_az * np.pi / 180.0, target_el, label=f"{target}", color=rgb)
            
            target_lst_az_el_zip = list(map(list, zip(target_lst, target_lst_az, target_lst_el)))
            target_lst_az_el_zip.sort()
            target_lst_az_el_zip = np.array(target_lst_az_el_zip)
            
            axs_lst[0].plot(target_lst_az_el_zip[:,0], target_lst_az_el_zip[:,1], label=f"{target}", color=rgb) # az
            axs_lst[1].plot(target_lst_az_el_zip[:,0], target_lst_az_el_zip[:,2], label=f"{target}", color=rgb) # el
            
        axs_azel[0].plot(target_datetime, sun_az, label="sun", color="k") # sun az
        axs_azel[1].plot(target_datetime, sun_el, label="sun", color="k") # sun el

        axs_azel[0].set_xlim(0,24)
        axs_azel[0].set_ylim(0,360)
        axs_azel[0].set_xticks(np.linspace(0,24,25))
        axs_azel[0].set_yticks(np.linspace(0,360,9))
        axs_azel[0].set_ylabel("AZ (deg)")
        axs_azel[0].legend(ncols=7, bbox_to_anchor=(0, 1.15), loc='upper left', borderaxespad=0, fontsize=10)
        axs_azel[1].set_xlim(0,24)
        axs_azel[1].set_ylim(0,90)
        axs_azel[1].set_xticks(np.linspace(0,24,25))
        axs_azel[1].set_yticks(np.linspace(0,90,10))
        axs_azel[1].set_xlabel(f"{date} UT")
        axs_azel[1].set_ylabel("EL (deg)")
        fig_azel.savefig(f"{save_pass2}/uptimeplot_azel_{date}_{antenna}.png")
        fig_azel.clf()

        axs_polar.set_rticks(np.linspace(0,90,10))
        axs_polar.set_rmax(0)
        axs_polar.set_rmin(90)
        axs_polar.set_theta_direction(-1)
        axs_polar.set_theta_offset(np.pi/2)
        axs_polar.legend(ncols=5, bbox_to_anchor=(0, 1.15), loc='upper left', borderaxespad=0, fontsize=10)
        fig_polar.savefig(f"{save_pass2}/uptimeplot_polar_{date}_{antenna}.png")
        fig_polar.clf()

        axs_lst[0].set_xlim(0,24)
        axs_lst[0].set_ylim(0,360)
        axs_lst[0].set_xticks(np.linspace(0,24,25))
        axs_lst[0].set_yticks(np.linspace(0,360,9))
        axs_lst[0].set_ylabel("AZ (deg)")
        axs_lst[0].legend(ncols=7, bbox_to_anchor=(0, 1.15), loc='upper left', borderaxespad=0, fontsize=10)
        axs_lst[1].set_xlim(0,24)
        axs_lst[1].set_ylim(0,90)
        axs_lst[1].set_xticks(np.linspace(0,24,25))
        axs_lst[1].set_yticks(np.linspace(0,90,10))
        axs_lst[1].set_xlabel(f"LST in {antenna}")
        axs_lst[1].set_ylabel("EL (deg)")
        fig_lst.savefig(f"{save_pass2}/uptimeplot_LST_{date}_{antenna}.png")
        fig_lst.clf()


        #plt.show()
        plt.close()
"""
import itertools
target_shuffle = itertools.combinations(target_list, 2)
fig_azel , axs_azel  = plt.subplots(2, 1, figsize=(12,9), sharex=True, tight_layout=True)
for target_pair in target_shuffle :
    az_diff = 0
    el_diff = 0
    for i, target in enumerate(target_pair) :
        factor = 0
        if i == 0 : factor = 1
        else :      factor = -1
        target_ra_h, target_ra_m, target_ra_s, target_dec_d, target_dec_m, target_dec_s = target_dict[target].split()
        target_ra  = Angle((float(target_ra_h) , float(target_ra_m) , float(target_ra_s)) , unit='hourangle').degree
        target_dec = Angle((float(target_dec_d), float(target_dec_m), float(target_dec_s)), unit=u.deg).degree
        target_datetime, target_az, target_el, _, _, _, _, _ = RaDec2AltAz(float(target_ra), float(target_dec), date, float(antenna_position_x), float(antenna_position_y), float(antenna_position_z))
        
        az_diff += factor * target_az
        el_diff += factor * target_el
        
    axs_azel[0].plot(target_datetime, az_diff, label=f"{target_pair[0]}-{target_pair[1]}") # az
    axs_azel[1].plot(target_datetime, el_diff, label=f"{target_pair[0]}-{target_pair[1]}") # el
axs_azel[0].set_xlim(0,24)
#axs_azel[0].set_ylim(0,360)
axs_azel[0].set_xticks(np.linspace(0,24,25))
axs_azel[0].set_ylabel("AZ (deg)")
axs_azel[0].legend(ncols=7, bbox_to_anchor=(0, 1.15), loc='upper left', borderaxespad=0, fontsize=10)
axs_azel[1].set_xlim(0,24)
axs_azel[1].set_xticks(np.linspace(0,24,25))
axs_azel[1].set_xlabel(f"{date} UT")
axs_azel[1].set_ylabel("EL (deg)")
fig_azel.savefig(f"{save_pass2}/uptimeplot_azel_diff_{date}_{antenna}.png")
#plt.show()
fig_azel.clf()
"""


# Executing the nautilus
import subprocess
subprocess.run([f"nautilus {save_pass1}"], shell=True)

