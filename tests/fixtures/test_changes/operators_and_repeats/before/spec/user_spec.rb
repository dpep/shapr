RSpec.describe User do
  it "finds" do
    expect(User.ids.sort == [1]).to be(true)
    expect(User.ids.sort.first).to eq(1)
  end
end
