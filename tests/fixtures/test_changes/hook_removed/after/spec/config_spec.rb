RSpec.describe Config do
  it "refuses" do
    expect { Config.reset }.to raise_error(Config::Error)
  end
end
