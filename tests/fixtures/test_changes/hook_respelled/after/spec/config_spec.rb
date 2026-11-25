RSpec.describe Config do
  before do
    Config.load(:all)
  end

  it "refuses" do
    expect { Config.reset }.to raise_error(Config::Error)
  end
end
