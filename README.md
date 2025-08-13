# uptimeplot
This program calculates the AZ/EL of target(s) you will observe with the Yamaguchi or Hitachi antennas (MIT License).

# uptimeplot-rs 
Rust version  
コンパイルは uptimeplot-rs ディレクトリの中で cargo run --release を実行する．そうすると，uptimeplot-rs/target/release/uptimeplot-rs が作成されるので，このシンボリックリンクをデスクトップなどの任意のディレクトリへ移動させることで利用できる．uptimeplot-rs は，そのディレクトリの中に保存されている station.txt と source.txt をコンパイル時に読み込んでいるので，それらのパスが変わるとプログラムの起動時に No file となる．その場合は Load Stations ボタンや Load Sources ボタンで任意のファイルをロードすればいい．Open Stations ボタンや Open Sources ボタンでファイルを開いて編集できる．いまのところは Win 11 と Ubuntu 24.04 LTS で動作確認済み． 

# uptimeplot.py
Python version
- You must edit the variable named "config=".
- You may improve this program if your PC is a Windows or Mac because I execute this program on Ubuntu 24.02 LTS.
- Please feel free to edit it.
