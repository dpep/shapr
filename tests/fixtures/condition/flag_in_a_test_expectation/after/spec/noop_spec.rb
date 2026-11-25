RSpec.describe Noop do
  it "yields" do
    expect(run).to eq(payload)
  end
end
