RSpec.describe User do
  it "finds by account" do
    found = User.ids
    expect(found).to eq(account)
  end
end
