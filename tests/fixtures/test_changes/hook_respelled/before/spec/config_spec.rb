RSpec.describe Config do
  before { Config.load }

  it "refuses" do
    expect { Config.reset }.to raise_error(Config::Error)
  end
end
